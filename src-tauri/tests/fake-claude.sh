#!/bin/sh
# Stand-in for `claude` in tests, reached through Roost's shim: prints its args,
# records its pid, fires hooks the way Claude would, then echoes lines until /exit.
echo "ARGS: $*"
echo $$ > .fake-claude-pid
hook() { printf '%s' "$2" | "$ROOST_BIN" hook "$1"; }
hook SessionStart "{\"session_id\":\"fake-claude-id\",\"cwd\":\"$(pwd -P)\"}"
sleep 0.3
hook UserPromptSubmit '{}'
sleep 0.3
hook Notification '{"notification_type":"permission_prompt"}'
sleep 0.3
hook PostToolUse '{}'
sleep 0.3
hook Stop '{}'
while IFS= read -r line; do
  echo "GOT: $line"
  case "$line" in *"/exit"*) hook SessionEnd '{}'; exit 0 ;; esac
done
