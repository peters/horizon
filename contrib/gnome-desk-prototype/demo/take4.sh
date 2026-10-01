#!/bin/bash
# One full take with the live voice agent. Horizon must be up (up.sh); the recorder runs alongside.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
cp $S/tools/demo/site/pricing-before.html $S/tools/demo/site/pricing.html
(setsid nohup python3 $S/record.py > $S/record.log 2>&1 &)
sleep 1
bash $S/story4.sh
touch $S/frames/STOP
sleep 3
cat $S/record.log
