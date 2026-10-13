#!/bin/sh
# Stand-in for `claude` in tests, reached through Roost's shim: prints its args,
# records its pid, fires hooks the way Claude would, then echoes lines until /exit.
echo "ARGS: $*"
echo $$ > .fake-claude-pid
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
while IFS= read -r line; do
  echo "GOT: $line"
  case "$line" in
    *"/exit"*) hook SessionEnd '{"reason":"prompt_input_exit"}'; exit 0 ;;
    *"/fail"*) hook StopFailure '{"error":"rate_limit"}' ;;
    *"/ask"*) hook PreToolUse '{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which database?"}]}}' ;;
    *"/turn"*) hook UserPromptSubmit "{\"prompt\":\"edit it\",\"cwd\":\"$(pwd -P)\"}"
               echo changed >> edited.txt
               hook Stop '{"last_assistant_message":"Edited edited.txt."}' ;;
    *"/work"*) hook UserPromptSubmit '{"prompt":"go"}' ;;
    *"/spin"*) printf '\033]0;\342\227\220 working\007' ;;
    *"/done"*) printf '\033]2;\342\234\263 working\007' ;;
    *"/clear"*) hook SessionEnd '{"reason":"clear"}'
                hook SessionStart "{\"source\":\"clear\",\"session_id\":\"fake-clear-id\",\"cwd\":\"$(pwd -P)\"}" ;;
  esac
done
