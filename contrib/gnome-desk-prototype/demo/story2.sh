#!/bin/bash
# The whole demo, timed. Run after Horizon has settled; the recorder runs alongside.
S=${DEMO_DIR:?set DEMO_DIR to a scratch directory}
C=$S/tools/demo/cmd.txt; : > $C; rm -f $C.log
c() { echo "$*" >> $C; }
sleep 2.0
c "go 2"; sleep 2.2
c "go 3"; sleep 2.2
c "go 4"; sleep 2.2
c "go 1"; sleep 2.4
c "voice $S/audio/request.env"
c "say 4900 Fix the failing API tests, update the pricing copy, and check the idle cloud environment."
sleep 5.8
c "enter"
sleep 10
c "go 2"; sleep 6.5
c "go 3"; sleep 6
c "go 1"; sleep 7.5
c "expand a"; sleep 4
c "expand b"; sleep 4
c "expand c"; sleep 4
c "collapse"; sleep 2
c "scope Cloud"; sleep 3
c "scope all"; sleep 2.5
