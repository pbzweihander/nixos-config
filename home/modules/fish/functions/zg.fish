function zg
    set -l root (git rev-parse --show-toplevel 2>/dev/null)

    if test -n "$root"
        cd "$root"
    else
        echo "Not inside a Git repository" >&2
        return 1
    end
end
