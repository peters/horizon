const selector = arguments[0], callback = arguments[arguments.length - 1];
// Keep page refusals distinct from a WebDriver protocol error envelope.
const done = geometry => callback({geometry});
const fail = (code, message) => done({error: {code, message}});
let element;
try { element = document.querySelector(selector); }
catch (_) { fail('invalid_selector', 'Invalid click selector'); return; }
if (!element) { fail('no_such_element', 'Click target no longer exists'); return; }
const started = performance.now();
let previous = null, stable = 0, scrolled = false;
function sample() {
    if (!element.isConnected || document.querySelector(selector) !== element) {
        fail('stale_reference', 'Click target changed while waiting'); return;
    }
    if (element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true') {
        fail('element_disabled', 'Click target is disabled'); return;
    }
    const rect = element.getClientRects()[0], style = getComputedStyle(element);
    if (!rect || rect.width <= 0 || rect.height <= 0 || style.visibility === 'hidden' || style.display === 'none') {
        fail('element_not_visible', 'Click target is not visible'); return;
    }
    const v = window.visualViewport;
    if (!v) { fail('viewport_unavailable', 'Visual viewport geometry is unavailable'); return; }
    const ox = v.offsetLeft, oy = v.offsetTop;
    const width = v.width, height = v.height;
    const left = Math.max(rect.left, ox), right = Math.min(rect.right, ox + width);
    const top = Math.max(rect.top, oy), bottom = Math.min(rect.bottom, oy + height);
    if (right <= left || bottom <= top) {
        if (performance.now() - started >= 2000) { fail('element_not_visible', 'Click target is outside the visual viewport'); return; }
        if (!scrolled) {
            element.scrollIntoView({block:'center', inline:'center', behavior:'instant'});
            scrolled = true;
        }
        previous = null; stable = 0;
        setTimeout(sample, 50); return;
    }
    const geometry = [rect.left, rect.top, rect.right, rect.bottom, ox, oy, width, height, v.scale, scrollX, scrollY];
    if (previous && geometry.every((value, i) => Math.abs(value - previous[i]) < 0.25)) stable++;
    else { stable = 0; previous = geometry; }
    if (stable >= 3) {
        // Validate the exact integer CSS point the native action will send.
        const x = Math.round((left + right) / 2 - ox) + ox;
        const y = Math.round((top + bottom) / 2 - oy) + oy;
        if (x < left || x >= right || y < top || y >= bottom) { fail('element_not_visible', 'Target has no integer click point'); return; }
        const hit = document.elementFromPoint(x, y);
        if (!hit || !element.contains(hit)) {
            // A nested scrollport can clip a rect inside the top-level viewport.
            if (!scrolled) {
                element.scrollIntoView({block:'center', inline:'center', behavior:'instant'});
                scrolled = true; previous = null; stable = 0;
                setTimeout(sample, 50); return;
            }
            fail('element_obscured', 'Another element covers the click point'); return;
        }
        done({x, y, offset_x:ox, offset_y:oy, width, height}); return;
    }
    if (performance.now() - started >= 2000) { fail('viewport_unstable', 'Click target or viewport is still moving'); return; }
    setTimeout(sample, 50);
}
sample();
