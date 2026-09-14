# Important `codex exec --json` events only, one line each. Run with `jq -n -r --unbuffered`.
# Dropped: successful commands, intermediate messages, per-file edits, searches.
# Everything stays in events.jsonl for review.
def short($n): gsub("\\s+"; " ") | sub("^ | $"; ""; "g") | if length > $n then .[:($n - 1)] + "…" else . end;
def unwrap:
  sub("^\\S*bash -lc '(?<b>.*)'$"; "\(.b)"; "p")
  | sub("^\\S*bash -lc \"(?<b>.*)\"$"; "\(.b)"; "p")
  | sub("^\\S*bash -lc (?<b>\\S+)$"; "\(.b)"; "p");
def lastline: [splits("\n") | select(test("\\S"))] | last // "";
def note: lastline | short(140) | if . == "" then "" else " :: " + . end;
def blocked: test("Operation not permitted|Permission denied|Read-only file system|Could not resolve|Temporary failure in name resolution|Name or service not known|Network is unreachable|EAI_AGAIN|ENOTFOUND"; "i");
# exit 1 from search/compare tools means "no match" or "differs", not an error
def nomatch: .exit_code == 1 and (.command | unwrap | test("(^|[|;&(]\\s*)(rg|grep|egrep|fgrep|test|\\[|diff|cmp)\\s"));

foreach inputs as $e (
  {cmds: 0, fails: 0, files: {}, seen: {}, plan: null, out: null};
  .out = null
  | ($e.item // {}) as $i
  | if $e.type == "item.completed" and $i.type == "command_execution" then
      .cmds += 1
      | if $i.exit_code == 0 or ($i | nomatch) then .
        else
          ($i.command | unwrap) as $c
          | ($i.aggregated_output // "") as $o
          | .fails += 1
          | .seen[$c] += 1
          | if ($o | blocked) then .out = "[BLOCKED rc=\($i.exit_code)] \($c | short(100))\($o | note)"
            elif .seen[$c] == 1 then .out = "[FAIL rc=\($i.exit_code)] \($c | short(100))\($o | note)"
            elif .seen[$c] == 3 then .out = "[STUCK?] same command failed 3 times: \($c | short(120))"
            else . end
        end
    elif $e.type == "item.completed" and $i.type == "file_change" then
      reduce ($i.changes[]?.path) as $path (.; .files[$path] = true)
    elif $i.type == "todo_list" then
      ([$i.items[] | select(.completed)] | length) as $d | ($i.items | length) as $n
      | if [$d, $n] != .plan then
          .plan = [$d, $n]
          | .out = "[plan \($d)/\($n)] next: " + ((first($i.items[] | select(.completed | not) | .text) // "all done") | short(120))
        else . end
    elif $e.type == "item.completed" and $i.type == "mcp_tool_call" and $i.status != "completed" then
      .out = "[FAIL mcp] \($i.server).\($i.tool)"
    elif $e.type == "item.completed" and $i.type == "error" then .out = "[ERROR] " + ($i.message | short(160))
    elif $e.type == "error" then .out = "[ERROR] " + (($e.message // "") | short(160))
    elif $e.type == "turn.completed" then
      .out = "[DONE] \(.cmds) cmds (\(.fails) failed), \(.files | length) files changed, tokens in=\($e.usage.input_tokens) out=\($e.usage.output_tokens)"
    elif $e.type == "turn.failed" then .out = "[FAILED] " + (($e.error.message // "") | short(160))
    else . end;
  .out // empty
)
