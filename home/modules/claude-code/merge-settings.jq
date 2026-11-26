# Only these exact commands belong to our previous generation. A user command
# containing the name (or an absolute/custom invocation) is deliberately retained.
def legacy_wiki_hook:
  .type == "command" and
  (.command as $command | ["claude-wiki context", "claude-wiki remind",
    "claude-wiki remind --event", "claude-wiki recall", "claude-wiki record",
    "agent-wiki context", "agent-wiki remind", "agent-wiki recall"] | index($command) != null);

def remove_legacy_hooks:
  if (.hooks | type) == "object" then
    .hooks |= with_entries(.value |= map(
      if (.hooks | type) == "array" and any(.hooks[]; legacy_wiki_hook) then
        .hooks |= map(select(legacy_wiki_hook | not)) | select(.hooks | length > 0)
      else . end))
  else . end;

def merge($b):
  if type == "object" and ($b | type) == "object" then
    reduce ($b | keys_unsorted[]) as $k (.; .[$k] |= merge($b[$k]))
  elif type == "array" and ($b | type) == "array" then
    reduce $b[] as $x (.; if index([$x]) then . else . + [$x] end)
  else $b end;

remove_legacy_hooks | merge($static[0])
