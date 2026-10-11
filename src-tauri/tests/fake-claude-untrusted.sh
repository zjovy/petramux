#!/bin/sh
# Stand-in for `claude` in a folder it doesn't trust yet: it shows the trust
# dialog (words positioned with cursor escapes, as real Claude draws them) and
# fires no hooks until someone answers.
printf 'Quick safety check: Is this a project you created or one you trust?\r\n'
printf '\033[4GNo,\033[8Gexit\r\n'
printf '\033[4GYes,\033[9GI\033[11Gtrust\033[17Gthis\033[22Gfolder\r\n'
exec cat
