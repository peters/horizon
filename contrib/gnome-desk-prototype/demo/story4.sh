#!/bin/bash
# The whole demo with the live voice agent, timed by events where the agent decides the pace.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
C=$S/tools/demo/cmd.txt; : > $C; rm -f $C.log $S/voice-out/*
c() { echo "$*" >> $C; }
wait_for() { local i=0; while ! grep -q "$1" $C.log 2>/dev/null; do sleep 0.5; i=$((i+1)); [ $i -ge $(($2*2)) ] && echo "timeout waiting for $1" >&2 && return 1; done; }
c "mark start"; sleep 3.6
c "mark tour"; c "go 2"; sleep 3.2
c "mark native"; c "go 4"; sleep 3.4
c "go 1"; sleep 1.2
c "mark mini_a"; c "mini a"; sleep 3.8
c "mark mini_b"; c "mini b"; sleep 3.8
c "mark mini_c"; c "mini c"; sleep 3.4
c "mark voice"; c "voice_go"
sleep 21
c "mark agents"; c "key right"; sleep 5.5; c "key right"; sleep 5.5; c "key left"; sleep 3.5; c "key left"; sleep 1
wait_for "event note" 100
sleep 1.5
c "mark asked"; sleep 2
c "mark feed"; c "expand a"; sleep 6
c "mark allow"; c "answer yes"; sleep 5.5
c "mark collapse"; c "collapse"; sleep 1.6
c "mark hubA"; c "hub a"; c "page hosts"; sleep 4.2
c "mark hubA_cloud"; c "page cloud"; sleep 4.4; c "page nav"; sleep 3.4; c "page close"; sleep 1
c "mark hubB"; c "hub b"; c "page sessions"; sleep 4.2; c "page settings"; sleep 3.4; c "page close"; sleep 1
c "mark hubC"; c "hub c"; c "page nav"; sleep 4.6; c "page close"; sleep 0.8; c "collapse"; sleep 1.2
c "mark spaces"; c "overview"; sleep 4.4; c "overview"; sleep 2.4
c "mark end"; sleep 0.5
