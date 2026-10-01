#!/bin/bash
# One full take: fresh shell and Horizon, record, play the story, stop.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
$S/gnome-restart.sh >/dev/null
$S/horizon-restart.sh 24 >/dev/null
(setsid nohup python3 $S/record.py > $S/record.log 2>&1 &)
sleep 1
bash $S/story2.sh
touch $S/frames/STOP
sleep 3
cat $S/record.log
