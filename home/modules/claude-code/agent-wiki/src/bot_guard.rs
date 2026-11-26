//! Bot-only turn anchors and advisory worktree cleanup. Never removes anything.
//! All scan/state I/O runs behind an 80 ms receive deadline; busy state fails open.
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs,
    io::{BufRead, BufReader, ErrorKind, Read},
    os::{
        fd::OwnedFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const ANCHOR: &str = "agent-wiki witch-anchor: 최종 응답(핸드오프 카드 포함)은 지아에게 직접 표시됩니다. 에이전트 원문이나 영문 상태 로그를 붙여넣지 말고, 이 위치의 한국어 페르소나로 핵심만 2~4줄로 간결하게 보고하세요(상세 리포트는 ~/.local/state/agent-wiki/artifacts/ 파일로 전달).";
const PREFIX: &str = "agent-wiki witch-cleanup: 정리 대상 임시 워크트리가 남아 있습니다: ";
const SUFFIX: &str =
    " 작업이 끝났다면 orca worktree rm --worktree path:<path> --delete-branch 로 정리하세요.";
/// Time for visiting worktrees. The rest of [`WAIT_BUDGET`] is left for recording
/// where the scan stopped (one small SQLite transaction).
const SCAN_BUDGET: Duration = Duration::from_millis(60);
const WAIT_BUDGET: Duration = Duration::from_millis(80);
/// Seconds between ordinary scans for one bot.
const THROTTLE_SECS: i64 = 900;
/// Seconds before retrying a scan that ran out of time.
const RETRY_SECS: i64 = 60;
/// Most leftovers named in one advisory.
const MAX_LEFT_WORKTREES: usize = 5;

fn bot_key(cwd: Option<&str>) -> Option<String> {
    let bot_home = std::env::var_os("ORCA_BOT_HOME").filter(|s| !s.is_empty());
    let path = bot_home
        .map(PathBuf::from)
        .or_else(|| cwd.filter(|s| !s.is_empty()).map(PathBuf::from))?;
    let path = crate::bulletin::resolve(&path)?;
    let home = crate::bulletin::resolve(&PathBuf::from(std::env::var_os("HOME")?))?;
    // Reuse bulletin's normalization, precedence and consumer identity, but only
    // accept this user's bot homes, not an arbitrary project's .orca/bots folder.
    path.strip_prefix(home.join(".orca/bots")).ok()?;
    crate::bulletin::consumer_key(None, Some(&path), "")
}

fn anchor(prompt: &str) -> bool {
    let mut envelope = false;
    for line in prompt.lines() {
        if line.trim_start().starts_with("──── From:") {
            envelope = true;
            if ["(handoff)", "(worker_done)", "(question)"]
                .iter()
                .any(|kind| line.contains(kind))
            {
                return true;
            }
        }
    }
    // A pure status/FYI envelope is not an owner request. Wrapped owner prompts
    // (including <USER_REQUEST>) are still owner requests.
    !envelope && !prompt.trim().is_empty()
}

pub(crate) fn prompt(root: &Path, hook: &crate::hook_payload::Hook) -> Vec<String> {
    // Antigravity calls this only after new_prompt accepted invocationNum == 1.
    if !matches!(
        hook.hook_event_name.as_str(),
        "UserPromptSubmit" | "PreInvocation"
    ) {
        return vec![];
    }
    let Some(key) = bot_key(hook.cwd.as_deref()) else {
        return vec![];
    };
    let prompt = hook.prompt.as_deref().unwrap_or_default();
    if prompt.trim().is_empty() {
        return vec![];
    }
    let mut messages = vec![];
    if anchor(prompt) {
        messages.push(ANCHOR.to_owned());
    }
    if let Some(note) = cleanup(root, key, prompt, 1500) {
        messages.push(note);
    }
    messages
}

pub(crate) fn session_start(root: &Path, cwd: Option<&str>, budget: usize) -> Option<String> {
    cleanup(root, bot_key(cwd)?, "", budget)
}

fn cleanup(root: &Path, key: String, prompt: &str, budget: usize) -> Option<String> {
    if budget < PREFIX.chars().count() + SUFFIX.chars().count() + 40 {
        return None;
    }
    let workspaces = crate::compat::var_os("AGENT_WIKI_WORKSPACES_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join("orca/workspaces")))?;
    let bypass = ["(worker_done)", "merged", "완료"]
        .iter()
        .any(|word| prompt.contains(word));
    let root = root.to_owned();
    let deadline = Instant::now() + SCAN_BUDGET;
    bounded(move || {
        let claim = claim(&root, &key, bypass).ok().flatten()?;
        let scan = scan(&workspaces, claim.cursor.as_deref(), deadline);
        // Best effort: a busy database loses only the cursor or the shorter retry.
        let _ = claim.finish(&key, &scan);
        let mut entries = vec![];
        for entry in scan.found {
            let mut candidate = entries.clone();
            candidate.push(entry);
            if format!("{PREFIX}{}{SUFFIX}", candidate.join("; "))
                .chars()
                .count()
                > budget
            {
                continue;
            }
            entries = candidate;
        }
        (!entries.is_empty()).then(|| format!("{PREFIX}{}{SUFFIX}", entries.join("; ")))
    })
}

fn bounded(work: impl FnOnce() -> Option<String> + Send + 'static) -> Option<String> {
    let started = Instant::now();
    let (send, receive) = mpsc::channel();
    thread::Builder::new()
        .name("wiki-bot-guard".into())
        .spawn(move || {
            let _ = send.send(work());
        })
        .ok()?;
    // This also bounds slow filesystem calls, not only git subprocesses. The
    // worker has its own deadline and never waits on SQLite locks or git pipes.
    receive
        .recv_timeout(WAIT_BUDGET.saturating_sub(started.elapsed()))
        .ok()
        .flatten()
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or_default()
}

/// A scan this process won the right to run, with where the last one stopped.
struct Claim {
    db: Connection,
    cursor: Option<String>,
}

impl Claim {
    /// Records the scan's end in at most one transaction, and none at all for the
    /// common scan that visits every worktree in time without a saved cursor.
    ///
    /// A scan cut off by the deadline moves the throttle stamp back so the next
    /// prompt after [`RETRY_SECS`] retries, instead of waiting the full
    /// [`THROTTLE_SECS`]. A scan that did not visit every worktree remembers where
    /// it stopped, so the next one continues there instead of revisiting the same
    /// early entries. A failed write (busy database) only loses these refinements.
    fn finish(&self, key: &str, scan: &Scan) -> rusqlite::Result<()> {
        let cursor = match (&scan.last, scan.finished) {
            (Some(last), false) => Some(Some(last)),
            (_, true) if self.cursor.is_some() => Some(None),
            _ => None,
        };
        if !scan.timed_out && cursor.is_none() {
            return Ok(());
        }
        let tx = self.db.unchecked_transaction()?;
        if scan.timed_out {
            tx.execute(
                "update bot_guard_state set last_worktree_check=? where consumer_key=?",
                params![now_secs() - THROTTLE_SECS + RETRY_SECS, key],
            )?;
        }
        match cursor {
            Some(Some(last)) => {
                tx.execute(
                    "insert into bot_guard_cursor values (?,?) on conflict(consumer_key) do update set after=excluded.after",
                    params![key, last],
                )?;
            }
            Some(None) => {
                tx.execute("delete from bot_guard_cursor where consumer_key=?", [key])?;
            }
            None => {}
        }
        tx.commit()
    }
}

/// Claims this bot's scan, or `None` while another scan is throttled or in progress.
fn claim(root: &Path, key: &str, bypass: bool) -> rusqlite::Result<Option<Claim>> {
    // Open directly: usage::open initializes unrelated transcript schemas and
    // permits a 50 ms busy wait per statement. This table is independently owned.
    let db = Connection::open(root.join("usage.sqlite"))?;
    db.busy_timeout(Duration::ZERO)?;
    db.execute_batch(
        "create table if not exists bot_guard_state(consumer_key text primary key, last_worktree_check integer not null);
        create table if not exists bot_guard_cursor(consumer_key text primary key, after text not null);",
    )?;
    let now = now_secs();
    // One atomic statement claims the scan across concurrent sessions. Bypass
    // also updates the timestamp, so the next ordinary prompt stays throttled.
    // A scan that then runs out of time shortens this (Claim::finish).
    let claimed = db.execute(
        "insert into bot_guard_state values (?1,?2) on conflict(consumer_key) do update set last_worktree_check=excluded.last_worktree_check where ?3 or bot_guard_state.last_worktree_check <= ?2-?4 or bot_guard_state.last_worktree_check > ?2",
        params![key, now, bypass, THROTTLE_SECS],
    )? != 0;
    if !claimed {
        return Ok(None);
    }
    let cursor = db
        .query_row(
            "select after from bot_guard_cursor where consumer_key=?",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(Some(Claim { db, cursor }))
}

fn regular(path: &Path) -> Option<fs::File> {
    fs::symlink_metadata(path).ok()?.is_file().then_some(())?;
    fs::File::open(path).ok()
}

fn small_file(path: &Path) -> Option<String> {
    let mut text = String::new();
    regular(path)?.take(8193).read_to_string(&mut text).ok()?;
    (text.len() <= 8192).then_some(text)
}

fn directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn sha(text: &str) -> Option<String> {
    let text = text.trim();
    (matches!(text.len(), 40 | 64) && text.bytes().all(|c| c.is_ascii_hexdigit()))
        .then(|| text.to_owned())
}

fn reference(common: &Path, name: &str, deadline: Instant) -> Option<String> {
    // HEAD must name a local branch; do not follow crafted ref paths or symlinks.
    if !name.starts_with("refs/")
        || name
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return None;
    }
    if let Some(value) = small_file(&common.join(name)).and_then(|s| sha(&s)) {
        return Some(value);
    }
    let file = regular(&common.join("packed-refs"))?;
    for line in BufReader::new(file.take(1024 * 1024)).lines().take(10_000) {
        if Instant::now() >= deadline {
            return None;
        }
        let line = line.ok()?;
        if let Some((value, found)) = line.split_once(' ')
            && found == name
        {
            return sha(value);
        }
    }
    None
}

fn reflog_entries(gitdir: &Path, deadline: Instant) -> Option<usize> {
    let file = regular(&gitdir.join("logs/HEAD"))?;
    let mut count = 0;
    for line in BufReader::new(file.take(64 * 1024)).lines().take(256) {
        if Instant::now() >= deadline {
            return None;
        }
        let line = line.ok()?;
        let mut fields = line.split_whitespace();
        let old = fields.next()?;
        let new = fields.next()?;
        sha(old)?;
        sha(new)?;
        // Git worktree add commonly writes creation + an initial no-op reset.
        // Counting that reset as work would flag every brand-new worktree.
        if count == 1 && old == new && line.ends_with("\treset: moving to HEAD") {
            continue;
        }
        count += 1;
        if count > 1 {
            break;
        }
    }
    Some(count)
}

/// Own the whole subprocess group, including helpers which inherit stdout after
/// git exits. Every return path signals the group and reaps the direct child.
struct GitProcess(Child);

impl Drop for GitProcess {
    fn drop(&mut self) {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // SAFETY: process_group(0) makes the spawned child's positive POSIX PID
        // its isolated PGID. Its negative value addresses only that group, never
        // our own. SIGKILL is 9 on Unix; no borrowed memory crosses this call.
        unsafe {
            kill(-(self.0.id() as i32), 9);
        }
        let _ = self.0.wait();
    }
}

fn git_ok(path: &Path, args: &[&str], empty_output: bool, deadline: Instant) -> Option<bool> {
    let deadline = deadline.min(Instant::now() + Duration::from_millis(20));
    if Instant::now() >= deadline {
        return None;
    }
    // A socket pair provides std-only nonblocking reads without platform-specific
    // fcntl constants or a new dependency. Descendants may inherit the writer;
    // neither the polling loop nor the final read ever waits for EOF.
    let (mut stdout, writer) = UnixStream::pair().ok()?;
    stdout.set_nonblocking(true).ok()?;
    let mut command = Command::new("git");
    // Do not inherit a caller's repository, index, object replacement or config
    // overrides. Disable optional index writes, fsmonitor and lazy network fetches.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let mut child = GitProcess(
        command
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
            ])
            .args(args)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .stderr(Stdio::null())
            .spawn()
            .ok()?,
    );
    // Command retains its stdio configuration: close the parent's writer too.
    drop(command);
    loop {
        let mut byte = [0];
        if empty_output {
            match stdout.read(&mut byte) {
                Ok(0) => {}
                Ok(_) => return Some(false),
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(_) => return None,
            }
        }
        match child.0.try_wait() {
            Ok(Some(status)) => {
                // Even a successful git may have left helpers behind. Kill them
                // before inspecting the remaining output, which is nonblocking.
                drop(child);
                if !status.success() {
                    return Some(false);
                }
                return Some(!empty_output || stdout.read(&mut byte).ok()? == 0);
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(1)),
            _ => return None,
        }
    }
}

fn candidate(path: &Path, deadline: Instant) -> Option<String> {
    let dotgit = path.join(".git");
    let gitdir = if directory(&dotgit) {
        dotgit
    } else {
        let text = small_file(&dotgit)?;
        path.join(text.trim().strip_prefix("gitdir: ")?)
    };
    if !directory(&gitdir) {
        return None;
    }
    let common = match small_file(&gitdir.join("commondir")) {
        Some(dir) => gitdir.join(dir.trim()),
        None => gitdir.clone(),
    };
    if !directory(&common) {
        return None;
    }
    let head = small_file(&gitdir.join("HEAD"))?;
    let head_ref = head.trim().strip_prefix("ref: refs/heads/")?;
    let head = reference(&common, &format!("refs/heads/{head_ref}"), deadline)?;
    let (main, main_name) = [
        ("refs/heads/main", "main"),
        ("refs/remotes/origin/main", "main"),
        ("refs/heads/master", "master"),
        ("refs/remotes/origin/master", "master"),
    ]
    .into_iter()
    .find_map(|(name, label)| reference(&common, name, deadline).map(|value| (value, label)))?;
    if head_ref == main_name {
        return None;
    }
    let entries = reflog_entries(&gitdir, deadline)?;
    let review =
        head_ref.starts_with("review-") || path.file_name()?.to_str()?.starts_with("review-");
    let stale = review
        && entries == 1
        && fs::metadata(gitdir.join("HEAD"))
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .ok()?
            > Duration::from_secs(7200);
    let reason = if stale {
        "stale review >2h".to_owned()
    } else if entries > 1
        && (head == main
            || git_ok(
                path,
                &["merge-base", "--is-ancestor", &head, &main],
                false,
                deadline,
            )?)
    {
        format!("merged into {main_name}")
    } else {
        return None;
    };
    if !git_ok(
        path,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ],
        true,
        deadline,
    )? {
        return None;
    }
    let path = path.to_str()?;
    // A filesystem or branch name must never inject a hook line, terminal or bidi
    // control, credential or instruction-like text: such a worktree is not named at
    // all, since a rewritten path would no longer identify it for removal.
    if !crate::bulletin::identifier_is_clean(path)
        || !crate::bulletin::identifier_is_clean(head_ref)
    {
        return None;
    }
    Some(format!("{path} ({head_ref}, {reason})"))
}

struct Scan {
    found: Vec<String>,
    /// Every candidate worktree was visited.
    finished: bool,
    /// The deadline ended the scan (or its listing) before it finished.
    timed_out: bool,
    /// Path of the last candidate visited in full.
    last: Option<String>,
}

/// Worktree directories under `root` (`<repo>/<worktree>`), sorted by path, or
/// `None` when the deadline passes while listing them.
fn worktrees(root: &Path, deadline: Instant) -> Option<Vec<PathBuf>> {
    let mut paths = vec![];
    if !directory(root) {
        return Some(paths);
    }
    let Ok(root) = root.canonicalize() else {
        return Some(paths);
    };
    let Ok(repos) = fs::read_dir(root) else {
        return Some(paths);
    };
    for repo in repos.take(256).flatten() {
        if Instant::now() >= deadline {
            return None;
        }
        if !repo.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(children) = fs::read_dir(repo.path()) else {
            continue;
        };
        paths.extend(
            children
                .take(1024)
                .flatten()
                .filter(|child| child.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|child| child.path()),
        );
    }
    paths.sort();
    Some(paths)
}

/// Visits worktrees in path order, starting after `cursor` (where the previous
/// scan stopped) and wrapping around, until all are visited, the deadline passes
/// or [`MAX_LEFT_WORKTREES`] leftovers are found. Starting after the cursor keeps
/// slow or persistently reported early entries from hiding later ones forever.
fn scan(root: &Path, cursor: Option<&str>, deadline: Instant) -> Scan {
    let mut scan = Scan {
        found: vec![],
        finished: false,
        timed_out: false,
        last: None,
    };
    let Some(paths) = worktrees(root, deadline) else {
        scan.timed_out = true;
        return scan;
    };
    let start = cursor.map_or(0, |cursor| {
        paths
            .iter()
            .position(|path| path.as_path() > Path::new(cursor))
            .unwrap_or(0)
    });
    let mut visited = 0;
    for path in paths[start..].iter().chain(&paths[..start]) {
        if Instant::now() >= deadline {
            scan.timed_out = true;
            break;
        }
        if scan.found.len() == MAX_LEFT_WORKTREES {
            break;
        }
        let entry = candidate(path, deadline);
        // A leftover is only reported after all its checks ran; no leftover past the
        // deadline may mean a check was cut off, so that worktree was not visited.
        if entry.is_none() && Instant::now() >= deadline {
            scan.timed_out = true;
            break;
        }
        visited += 1;
        scan.last = Some(path.to_string_lossy().into_owned());
        scan.found.extend(entry);
    }
    scan.finished = visited == paths.len();
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_actionable_envelopes_but_not_status() {
        assert!(anchor("<USER_REQUEST>검토해 줘</USER_REQUEST>"));
        assert!(!anchor(""));
        assert!(!anchor("──── From: bot-2 (status) ────\n참고용 완료 보고"));
        for kind in ["handoff", "worker_done", "question"] {
            assert!(anchor(&format!("──── From: bot-2 ({kind}) ────\n내용")));
        }
    }

    #[test]
    fn blocked_scan_returns_within_one_hundred_ms() {
        let start = Instant::now();
        assert!(
            bounded(|| {
                thread::sleep(Duration::from_millis(250));
                Some("too late".into())
            })
            .is_none()
        );
        assert!(start.elapsed() < Duration::from_millis(100));
    }
}
