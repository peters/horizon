#!/bin/bash
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
export PYTHONPATH=${TTS_SITE:-}
cd $S/video
python3 plan.py > plan.log 2>&1 && python3 audio.py > audio.log 2>&1 && python3 compose.py $S/video/horizon-desk-demo.mp4 > render.log 2>&1
echo finished >> render.log
