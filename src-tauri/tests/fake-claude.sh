#!/bin/sh
# Stand-in for `claude` in tests: prints its args, fires hooks the way Claude
# would (through the real `roost hook`), then echoes whatever it is sent.
echo "ARGS: $*"
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
exec cat
