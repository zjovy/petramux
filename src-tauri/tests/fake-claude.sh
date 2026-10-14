#!/bin/sh
# Stand-in for `claude` in tests, reached through Roost's shim: prints its args,
# records its pid, fires hooks the way Claude would, then echoes lines until /exit.
# Claude's background service: start, stop and remove a job (its record in ~/.claude/jobs).
case "$1" in
  --bg)
    mkdir -p "$HOME/.claude/jobs/fakebg01"
    printf '{"name":"%s","state":"working","tempo":"active","sessionId":"fakebg-convo","cwd":"%s"}' "$2" "$(pwd -P)" > "$HOME/.claude/jobs/fakebg01/state.json"
    echo "backgrounded · fakebg01"
    echo "  claude attach fakebg01    open in this terminal"
    exit 0 ;;
  stop)
    sed -i '' 's/"state":"working"/"state":"stopped"/' "$HOME/.claude/jobs/$2/state.json" && echo "stopped $2"
    exit 0 ;;
  rm)
    [ "$2" = keep ] && { echo "has unpushed commits: pass --discard-unpushed abc@wt to remove anyway"; exit 1; }
    rm -rf "$HOME/.claude/jobs/$2" && echo "removed $2"
    exit 0 ;;
esac
# `claude attach <job>`: Claude's background view fires no hooks; it shows the job's name in the title.
if [ "$1" = attach ]; then
  echo "ARGS: $*"
  printf '\033]0;\342\234\263 attached\007'
  while IFS= read -r line; do case "$line" in *"/exit"*) exit 0 ;; esac; done
  exit 0
fi
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
# Like Claude: it draws again when the size it finds differs from the one it drew at. Two size
# changes in a row arrive as one signal, and then the size it finds may be the one it drew at.
drawn=$(stty size 2>/dev/null)
trap 'now=$(stty size 2>/dev/null); if [ "$now" != "$drawn" ]; then drawn=$now; echo REDRAWN; fi' WINCH
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
    *"/plaintitle"*) printf '\033]0;zsh\007' ;;
    *"/spin"*) printf '\033]0;\342\227\220 working\007' ;;
    *"/done"*) printf '\033]2;\342\234\263 working\007' ;;
    *"/clear"*) hook SessionEnd '{"reason":"clear"}'
                hook SessionStart "{\"source\":\"clear\",\"session_id\":\"fake-clear-id\",\"cwd\":\"$(pwd -P)\"}" ;;
  esac
done
