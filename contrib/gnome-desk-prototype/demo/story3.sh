#!/bin/bash
# The whole demo, timed. Run after Horizon has settled; the recorder runs alongside.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
C=$S/tools/demo/cmd.txt; : > $C; rm -f $C.log
c() { echo "$*" >> $C; }
c "mark start"; sleep 2.6
c "mark tour"; c "go 2"; sleep 3.4
c "mark native"; c "go 4"; sleep 3.6
c "mark keys"; c "key right"; sleep 2.2; c "key left"; sleep 2.2
c "go 1"; sleep 1.6
c "mark overview"; c "overview"; sleep 4.2
c "mark overview_close"; c "overview"; sleep 2.2
c "mark voice"; c "voice $S/audio/request.env"
c "say 4900 Fix the failing API tests, update the pricing copy, and check the idle cloud environment."
sleep 5.8
c "mark enter"; c "enter"
sleep 10
c "mark work2"; c "key right"; sleep 6.5
c "mark work3"; c "key right"; sleep 6
c "mark work1"; c "go 1"; sleep 7.5
c "mark expA"; c "expand a"; sleep 4
c "mark expB"; c "expand b"; sleep 4
c "mark expC"; c "expand c"; sleep 4
c "mark collapse"; c "collapse"; sleep 2.4
c "mark move"; c "go 2"; sleep 2.4
c "move 4 pricing preview"; sleep 3
c "mark moved"; c "go 4"; sleep 4.2
c "mark scope"; c "scope Cloud"; sleep 3
c "scope all"; sleep 2.4
c "mark end"; sleep 0.5
