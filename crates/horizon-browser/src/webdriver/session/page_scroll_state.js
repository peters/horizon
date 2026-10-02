const root = document.scrollingElement || document.documentElement;
const width = window.innerWidth, height = window.innerHeight;
// Screenshots leave native scrollbar gutters blank. Hit-test a viewport grid
// and walk up from each topmost element so only visible, unoccluded scroll
// containers report the gutter the host paints over.
const nested = [], seen = new Set(), grid = 8, limit = 16;
const parentOf = node => node.parentElement || (node.getRootNode().host ?? null);
const topmost = (x, y) => {
    let node = document.elementFromPoint(x, y);
    while (node && node.shadowRoot) {
        const inner = node.shadowRoot.elementFromPoint(x, y);
        if (!inner || inner === node) break;
        node = inner;
    }
    return node;
};
// Content is clipped out of the gutter, so a hit there is the scrollbar only
// when it is the container itself; any descendant hit overlays the gutter.
const owns = (node, x, y) => topmost(x, y) === node;
// Client metrics map onto the viewport only through translation.
const translatedOnly = style => {
    if ((style.rotate || 'none') !== 'none' || (style.scale || 'none') !== 'none') return false;
    if (style.transform === 'none') return true;
    const matrix = new DOMMatrixReadOnly(style.transform);
    return matrix.is2D && matrix.a === 1 && matrix.b === 0 && matrix.c === 0 && matrix.d === 1;
};
// Bisect between an occluded and a visible probe to the nearest pixel.
const boundary = (node, x, hidden, shown) => {
    while (Math.abs(shown - hidden) > 1) {
        const middle = (hidden + shown) / 2;
        if (owns(node, x, middle)) shown = middle; else hidden = middle;
    }
    return shown;
};
// First visible probe walking from `from` toward `to` in `step`s, refined to a pixel.
const firstVisible = (node, x, from, to, step) => {
    if (owns(node, x, from)) return from;
    for (let hidden = from; ; hidden += step) {
        const reached = step > 0 ? hidden + step >= to : hidden + step <= to;
        const probe = reached ? to : hidden + step;
        if (owns(node, x, probe)) return boundary(node, x, hidden, probe);
        if (reached) return null;
    }
};
// Vertical span of the gutter that ancestors, the viewport and overlays leave
// visible, or null when an ancestor rotates or scales the container.
const visibleSpan = (node, x, top, bottom) => {
    for (let clip = parentOf(node); clip && clip !== root; clip = parentOf(clip)) {
        const style = getComputedStyle(clip);
        if (!translatedOnly(style)) return null;
        if (style.overflowY === 'visible') continue;
        const box = clip.getBoundingClientRect();
        top = Math.max(top, box.top + clip.clientTop);
        bottom = Math.min(bottom, box.top + clip.clientTop + clip.clientHeight);
    }
    top = Math.max(top, 0);
    bottom = Math.min(bottom, height);
    if (bottom - top < 2) return null;
    const step = (bottom - top) / 8;
    const first = firstVisible(node, x, top + 0.5, bottom - 0.5, step);
    if (first === null) return null;
    const last = firstVisible(node, x, bottom - 0.5, first, -step);
    return last === null || last - first < 1 ? null : [first - 0.5, last + 0.5];
};
for (let row = 0; row < grid && nested.length < limit; row++) {
    for (let column = 0; column < grid && nested.length < limit; column++) {
        let node = topmost((column + 0.5) * width / grid, (row + 0.5) * height / grid);
        for (; node && node !== root && nested.length < limit; node = parentOf(node)) {
            if (seen.has(node)) break;
            seen.add(node);
            if (!(node instanceof HTMLElement)) continue;
            if (node.clientHeight < 32 || node.scrollHeight <= node.clientHeight + 1) continue;
            const style = getComputedStyle(node);
            if (!['auto', 'scroll', 'overlay'].includes(style.overflowY)) continue;
            if (!translatedOnly(style)) continue;
            const rect = node.getBoundingClientRect();
            const borderLeft = parseFloat(style.borderLeftWidth) || 0;
            const borderRight = parseFloat(style.borderRightWidth) || 0;
            let gutter = node.offsetWidth - node.clientWidth - borderLeft - borderRight;
            if ((style.scrollbarGutter || '').includes('both-edges')) gutter /= 2;
            if (!(gutter >= 1)) continue;
            const trackX = style.direction === 'rtl' ? rect.left + borderLeft : rect.right - borderRight - gutter;
            const trackY = rect.top + node.clientTop;
            const span = visibleSpan(node, trackX + gutter / 2, trackY, trackY + node.clientHeight);
            if (!span) continue;
            nested.push({
                track_x: trackX,
                track_y: trackY,
                track_width: gutter,
                track_height: node.clientHeight,
                visible_top: span[0],
                visible_bottom: span[1],
                scroll_top: node.scrollTop,
                scroll_height: node.scrollHeight,
            });
        }
    }
}
return {
    scroll_x: window.scrollX, scroll_y: window.scrollY,
    viewport_width: width, viewport_height: height,
    client_width: document.documentElement.clientWidth, client_height: document.documentElement.clientHeight,
    content_width: root.scrollWidth, content_height: root.scrollHeight,
    nested: nested.filter(bar => Object.values(bar).every(Number.isFinite)),
};
