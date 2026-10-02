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
// Hit testing ignores paint: a transparent element still wins the hit.
const transparency = new Map();
const paintsNothing = element => {
    if (!element) return false;
    if (!transparency.has(element)) {
        transparency.set(element, getComputedStyle(element).opacity === '0' || paintsNothing(parentOf(element)));
    }
    return transparency.get(element);
};
// Content is clipped out of the gutter, so a hit there is the scrollbar only
// when it is the container itself; any other painted hit overlays the gutter.
// Overlays with `pointer-events: none` are invisible to every hit-test API.
const owns = (node, x, y) => {
    const hit = topmost(x, y);
    if (hit === node) return true;
    if (!hit || !paintsNothing(hit)) return false;
    for (const element of node.getRootNode().elementsFromPoint(x, y)) {
        if (element === node) return true;
        if (!paintsNothing(element)) return false;
    }
    return false;
};
// Client metrics map onto the viewport only through translation: rotation,
// scale and CSS zoom all change the box without changing those metrics.
const translatedOnly = style => {
    if ((style.rotate || 'none') !== 'none' || (style.scale || 'none') !== 'none') return false;
    const zoom = parseFloat(style.zoom);
    if (Number.isFinite(zoom) && zoom !== 1) return false;
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
// Vertical runs of the gutter that ancestors, the viewport and overlays leave
// visible; none when an ancestor or the root rotates, scales or zooms it.
const visibleSpans = (node, x, top, bottom) => {
    for (let clip = parentOf(node); clip && clip !== root; clip = parentOf(clip)) {
        const style = getComputedStyle(clip);
        if (!translatedOnly(style)) return [];
        if (style.overflowY === 'visible') continue;
        const box = clip.getBoundingClientRect();
        top = Math.max(top, box.top + clip.clientTop);
        bottom = Math.min(bottom, box.top + clip.clientTop + clip.clientHeight);
    }
    for (const outer of new Set([root, document.documentElement])) {
        if (!translatedOnly(getComputedStyle(outer))) return [];
    }
    top = Math.max(top, 0) + 0.5;
    bottom = Math.min(bottom, height) - 0.5;
    if (bottom - top < 1) return [];
    const probes = 16, step = (bottom - top) / probes, spans = [];
    let start = null;
    for (let index = 0; index <= probes; index++) {
        const y = top + index * step;
        const shown = owns(node, x, y);
        if (shown && start === null) start = index === 0 ? y : boundary(node, x, y - step, y);
        if (!shown && start !== null) {
            spans.push([start, boundary(node, x, y, y - step)]);
            start = null;
        }
    }
    if (start !== null) spans.push([start, bottom]);
    return spans.filter(([first, last]) => last - first >= 1).map(([first, last]) => [first - 0.5, last + 0.5]);
};
// A page that breaks the nested scan must not cost the root sample.
try {
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
                // Reverse-flow scrollers report 0 at the bottom and negative offsets above it.
                const reversed = style.display.includes('flex') && style.flexDirection === 'column-reverse';
                const scrollTop = reversed ? node.scrollHeight - node.clientHeight + node.scrollTop : node.scrollTop;
                for (const [visibleTop, visibleBottom] of visibleSpans(node, trackX + gutter / 2, trackY, trackY + node.clientHeight)) {
                    nested.push({
                        track_x: trackX,
                        track_y: trackY,
                        track_width: gutter,
                        track_height: node.clientHeight,
                        visible_top: visibleTop,
                        visible_bottom: visibleBottom,
                        scroll_top: scrollTop,
                        scroll_height: node.scrollHeight,
                    });
                }
            }
        }
    }
} catch (_) {
    nested.length = 0;
}
return {
    scroll_x: window.scrollX, scroll_y: window.scrollY,
    viewport_width: width, viewport_height: height,
    client_width: document.documentElement.clientWidth, client_height: document.documentElement.clientHeight,
    content_width: root.scrollWidth, content_height: root.scrollHeight,
    nested: nested.filter(bar => Object.values(bar).every(Number.isFinite)),
};
