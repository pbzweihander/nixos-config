# Turn `codex exec --json` events into one short progress line each. Used by codex-monitor.sh.
def short($n): gsub("\\s+"; " ") | sub("^ | $"; ""; "g") | if length > $n then .[:($n - 1)] + "…" else . end;
def unwrap:
  sub("^\\S*bash -lc '(?<b>.*)'$"; "\(.b)"; "p")
  | sub("^\\S*bash -lc \"(?<b>.*)\"$"; "\(.b)"; "p")
  | sub("^\\S*bash -lc (?<b>\\S+)$"; "\(.b)"; "p");
.type as $t | (.item // {}) as $i | $i.type as $k |
if $t == "item.completed" and $k == "agent_message" then "[msg] " + ($i.text | short(160))
elif $t == "item.completed" and $k == "command_execution" then
  (if $i.exit_code == 0 then "[cmd rc=0] " else "[cmd FAIL rc=\($i.exit_code)] " end) + ($i.command | unwrap | short(120))
elif $t == "item.completed" and $k == "file_change" then "[edit] " + ([$i.changes[] | "\(.kind) \(.path)"] | join(", "))
elif $k == "todo_list" then
  "[plan \([$i.items[] | select(.completed)] | length)/\($i.items | length)] " + ([$i.items[].text] | join(" | ") | short(160))
elif $t == "item.completed" and $k == "web_search" then "[search] " + ($i.query | short(160))
elif $t == "item.completed" and $k == "mcp_tool_call" then "[mcp \($i.status)] \($i.server).\($i.tool)"
elif $t == "item.completed" and $k == "error" then "[ERROR] " + ($i.message | short(160))
elif $t == "turn.completed" then "[DONE] turn completed, in=\(.usage.input_tokens) out=\(.usage.output_tokens)"
elif $t == "turn.failed" then "[FAILED] " + ((.error.message // "") | short(160))
elif $t == "error" then "[ERROR] " + ((.message // "") | short(160))
else empty end
