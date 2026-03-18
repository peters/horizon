#!/bin/sh

set -eu

cycles="${HORIZON_REPRO_CYCLES:-120}"
visible_rows="${HORIZON_REPRO_ROWS:-18}"
blank_delay="${HORIZON_REPRO_BLANK_DELAY:-0.012}"
frame_delay="${HORIZON_REPRO_FRAME_DELAY:-0.045}"
sync_updates="${HORIZON_REPRO_SYNC_UPDATES:-0}"

begin_sync() {
    if [ "$sync_updates" = "1" ]; then
        printf '\033[?2026h'
    fi
}

end_sync() {
    if [ "$sync_updates" = "1" ]; then
        printf '\033[?2026l'
    fi
}

cleanup() {
    printf '\033[0m\033[?25h\033[?1049l'
}

trap cleanup EXIT INT TERM HUP

printf '\033[?1049h\033[?25l'

step=1
while [ "$step" -le "$cycles" ]; do
    begin_sync

    # Clear first, then redraw the prompt/footer before the candidate list.
    printf '\033[H\033[2J'
    printf '\033[1;1H issue-19 redraw repro\033[K'
    printf '\033[2;1H sync-updates=%s  blank-delay=%ss  frame-delay=%ss\033[K' \
        "$sync_updates" "$blank_delay" "$frame_delay"
    printf '\033[22;1H query> redraw-cycle-%03d\033[K' "$step"
    printf '\033[23;1H %02d matches  macOS shell panel  transcript wrapper active\033[K' "$visible_rows"
    printf '\033[24;1H ctrl-c exits\033[K'

    sleep "$blank_delay"

    highlight_row=$((step % visible_rows + 1))
    row=1
    while [ "$row" -le "$visible_rows" ]; do
        screen_row=$((row + 3))
        if [ "$row" -eq "$highlight_row" ]; then
            printf '\033[%d;1H\033[7m %02d  candidate %02d for redraw cycle %03d                       \033[0m\033[K' \
                "$screen_row" "$row" "$row" "$step"
        else
            printf '\033[%d;1H %02d  candidate %02d for redraw cycle %03d                         \033[K' \
                "$screen_row" "$row" "$row" "$step"
        fi
        row=$((row + 1))
    done

    end_sync
    sleep "$frame_delay"
    step=$((step + 1))
done

sleep 1
