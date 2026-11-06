#!/bin/zsh
# Screenshot the running app's main window without focusing it.
#   tools/shot.sh out.png [process-name]
# Builds tools/winid (CGWindowList lookup) on first use.
set -e
here=${0:A:h}
[[ -x $here/winid ]] || swiftc -O $here/winid.swift -o $here/winid
name=${2:-reel-studio-gpui}
id=$($here/winid $name | sort -k4 -n -r | awk 'NR == 1 {print $1}')
[[ -n $id ]] || { echo "no $name window" >&2; exit 1; }
screencapture -x -o -l $id ${1:-shot.png}
echo ${1:-shot.png}
