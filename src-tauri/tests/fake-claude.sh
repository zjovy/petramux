#!/bin/sh
# Stand-in for `claude` in tests, reached through Roost's shim: prints its args,
# records its pid, fires hooks the way Claude would, then echoes lines until /exit.
echo "ARGS: $*"
echo $$ > .fake-claude-pid
# Like Claude (watching from the start, as Claude does): it draws again when the size it finds differs from the one it drew at. Two size
# changes in a row arrive as one signal, and then the size it finds may be the one it drew at.
drawn=$(stty size 2>/dev/null)
trap 'now=$(stty size 2>/dev/null); if [ "$now" != "$drawn" ]; then drawn=$now; echo REDRAWN; fi' WINCH
hook() { printf '%s' "$2" | "$ROOST_BIN" hook "$1"; }
# Like Claude: a terminal that hangs up ends the session with a SessionEnd hook.
trap 'hook SessionEnd "{\"reason\":\"other\"}"; exit 0' HUP
# Real Claude saves a conversation at its first prompt; this one counts as saved from the start.
mkdir -p "$HOME/.claude/projects/fake" && : > "$HOME/.claude/projects/fake/fake-claude-id.jsonl"
hook SessionStart "{\"session_id\":\"fake-claude-id\",\"cwd\":\"$(pwd -P)\"}"
sleep 0.3
hook UserPromptSubmit '{}'
sleep 0.3
hook Notification '{"notification_type":"permission_prompt"}'
sleep 0.3
hook PostToolUse '{}'
sleep 0.3
hook Stop '{}'
# (a read cut short by that signal isn't the end of input)
while IFS= read -r line || [ $? -gt 128 ]; do
  echo "GOT: $line"
  case "$line" in
    *"/exit"*) hook SessionEnd '{"reason":"prompt_input_exit"}'; exit 0 ;;
    *"/fail"*) hook StopFailure '{"error":"rate_limit"}' ;;
    *"/ask"*) hook PreToolUse '{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which database?"}]}}' ;;
    *"/turn"*) hook UserPromptSubmit "{\"prompt\":\"edit it\",\"cwd\":\"$(pwd -P)\"}"
               echo changed >> edited.txt
               hook Stop '{"last_assistant_message":"Edited edited.txt."}' ;;
    *"/work"*) hook UserPromptSubmit '{"prompt":"go"}' ;;
    *"/clear"*) hook SessionEnd '{"reason":"clear"}'
                hook SessionStart "{\"source\":\"clear\",\"session_id\":\"fake-clear-id\",\"cwd\":\"$(pwd -P)\"}" ;;
  esac
done
