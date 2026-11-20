#include "fw_keyframe.h"

#include <string.h>

static int upper_median(const uint16_t *values, unsigned count) {
    uint16_t sorted[30];
    for (unsigned i = 0; i < count; ++i) {
        unsigned j = i;
        while (j && sorted[j - 1] > values[i]) {
            sorted[j] = sorted[j - 1];
            --j;
        }
        sorted[j] = values[i];
    }
    return sorted[count / 2];
}

static unsigned distance(int a, int b) {
    return (unsigned)(a < b ? b - a : a - b);
}

fw_keyframe_result_t fw_keyframe_poll(fw_keyframe_detector_t *d, const uint16_t raw[112], uint32_t now_ms) {
    fw_keyframe_result_t old = d->frame;
    if (d->last_poll_ms && now_ms - d->last_poll_ms < 100U) {
        old.changed = old.calibrate = false;
        return old;
    }
    d->last_poll_ms = now_ms;
    const uint16_t id[2] = {raw[5], raw[106]};
    d->history[0][d->next] = id[0];
    d->history[1][d->next] = id[1];
    d->next = (d->next + 1U) % 30U;
    if (d->samples < 30)
        d->samples++;

    unsigned deviation[2] = {0}, age[2] = {30, 30};
    for (int i = 0; i < 2; i++) {
        if (d->samples < 30)
            continue;
        int median = upper_median(d->history[i], 30);
        for (unsigned j = 0; j < 30; j++) {
            unsigned delta = distance(d->history[i][j], median);
            if (deviation[i] < delta)
                deviation[i] = delta;
            unsigned since = (d->next + 59U - j) % 30U;
            if (delta > 31 && age[i] > since)
                age[i] = since;
        }
    }

    bool one_moved = old.attached && ((deviation[0] > 250) != (deviation[1] > 250));
    uint8_t polarity[2] = {id[0] > 2540 ? 1 : id[0] < 2037 ? 2 : 0, id[1] > 2020 ? 1 : id[1] < 1518 ? 2 : 0};
    for (int i = 0; i < 2; i++) {
        if (deviation[i] > 31) {
            if (deviation[i] > 250 && !one_moved && (!old.attached || d->stable_id[i] != polarity[i]))
                d->stable_id[i] = 0;
        } else if (polarity[i]) {
            d->stable_id[i] = polarity[i];
        } else if ((d->stable_id[i] == 1 && id[i] <= (i ? 1919 : 2439)) ||
                   (d->stable_id[i] == 2 && id[i] > (i ? 1618 : 2137))) {
            d->stable_id[i] = 0;
        }
    }

    unsigned moving = 0;
    for (int i = 0; i < 112; ++i) {
        if (i != 5 && i != 106 && distance(raw[i], d->previous[i]) > 60)
            moving++;
    }
    memcpy(d->previous, raw, sizeof(d->previous));
    d->stable_scans = moving > 1 ? 0 : d->stable_scans < 255 ? d->stable_scans + 1 : 255;

    int left = upper_median(raw, 5), right = upper_median(raw + 107, 5);
    bool field = old.attached ? left > 2100 && right <= 1899 : left > 2200 && right < 1800;
    uint8_t layout = field ? d->stable_id[0] : d->stress_frame ? 1 : 0;
    uint8_t feel = field ? d->stable_id[1] : d->stress_frame ? 1 : 0;
    fw_keyframe_result_t next = old;
    if (layout && feel) {
        if (old.attached || d->stable_scans >= 3 || d->wait_scans >= 10) {
            next.attached = 1;
            next.layout = layout;
            next.feel = feel;
            d->missing_scans = d->wait_scans = 0;
        } else {
            d->wait_scans++;
        }
    } else if (!old.attached || ++d->missing_scans >= 4) {
        next.attached = next.layout = next.feel = 0;
        d->missing_scans = 0;
        if (!old.attached)
            d->wait_scans = 0;
    }
    next.stage = next.attached ? 3 : !field ? 0 : polarity[0] && polarity[1] && age[0] > 9 && age[1] > 9 ? 2 : 1;
    next.changed = next.attached != old.attached || next.layout != old.layout || next.feel != old.feel;
    next.calibrate = next.changed && next.attached;
    d->frame = next;
    return next;
}
