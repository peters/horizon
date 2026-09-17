const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../../crates/horizon-browser/src/webdriver/session/remote_click.js'), 'utf8');

function probe(options = {}) {
    let now = 0, result, scrolls = 0;
    const timers = [], hitPoints = [];
    const viewport = {offsetLeft:20, offsetTop:172, width:300, height:434, scale:1};
    let rect = {left:30, top:200, right:130, bottom:220, width:100, height:20};
    const element = {
        isConnected:true,
        matches: () => !!options.disabled,
        getAttribute: () => null,
        getClientRects: () => [options.rect ? options.rect(now) : rect],
        contains: hit => hit === element,
        scrollIntoView: () => { scrolls++; rect = {...rect, top:200, bottom:220}; },
    };
    if (options.offscreen) rect = {...rect, top:1200, bottom:1220};
    const context = {
        document: {
            querySelector: () => options.stale && now >= 100 ? null : element,
            elementFromPoint: (x, y) => {
                hitPoints.push([x, y]);
                const current = options.rect ? options.rect(now) : rect;
                const inside = x >= current.left && x < current.right && y >= current.top && y < current.bottom
                    && x >= viewport.offsetLeft && x < viewport.offsetLeft + viewport.width
                    && y >= viewport.offsetTop && y < viewport.offsetTop + viewport.height;
                return inside && !options.obscured && !(options.nestedClip && scrolls === 0) ? element : {};
            },
        },
        window: {visualViewport:viewport},
        getComputedStyle: () => ({display:options.hidden ? 'none' : 'block', visibility:'visible'}),
        performance: {now:() => now},
        scrollX:0, scrollY:0, innerWidth:400, innerHeight:800,
        setTimeout: (callback, delay) => timers.push([callback, now + delay]),
    };
    const run = vm.runInNewContext('(function(){' + source + '})', context);
    run('#target', value => { result = JSON.parse(JSON.stringify(value.geometry)); });
    while (!result && timers.length) {
        const [callback, when] = timers.shift(); now = when;
        if (now > 2500) throw new Error('unbounded geometry wait');
        if (options.viewport) Object.assign(viewport, options.viewport(now));
        callback();
    }
    return {result, elapsed:now, scrolls, hitPoints};
}

test('already visible keyboard-open targets settle without scrolling', () => {
    const {result, elapsed, scrolls, hitPoints} = probe();
    assert.equal(result.x, 80);
    assert.equal(result.y, 210);
    assert.equal(result.offset_y, 172);
    assert.deepEqual(hitPoints, [[80, 210]]);
    assert.equal(scrolls, 0);
    assert.ok(elapsed >= 150);
});

test('fractional viewport offsets hit-test the rounded native point in layout coordinates', () => {
    const {result, hitPoints} = probe({viewport:() => ({offsetLeft:20.4, offsetTop:172.4})});
    assert.deepEqual(hitPoints, [[80.4, 210.4]]);
    assert.equal(Math.round(result.x - result.offset_x), 60);
    assert.equal(Math.round(result.y - result.offset_y), 38);
});

test('offscreen targets are scrolled once, then remeasured', () => {
    const {result, scrolls} = probe({offscreen:true});
    assert.equal(scrolls, 1);
    assert.equal(result.y, 210);
});

test('ancestor scrollport clipping is exposed before a click point is returned', () => {
    const {result, elapsed, scrolls} = probe({nestedClip:true});
    assert.equal(scrolls, 1);
    assert.equal(result.y, 210);
    assert.ok(elapsed >= 350);
});

test('keyboard movement and zoom restart the stability interval', () => {
    const {result, elapsed} = probe({viewport:now => ({offsetTop:now < 400 ? 172 + now / 20 : 192, scale:now < 300 ? 1 : 1.5})});
    assert.equal(result.offset_y, 192);
    assert.ok(elapsed >= 550);
});

test('continuous movement is refused within a bounded interval', () => {
    const {result, elapsed} = probe({rect:now => ({left:30, right:130, top:200 + now / 100, bottom:220 + now / 100, width:100, height:20})});
    assert.equal(result.error.code, 'viewport_unstable');
    assert.equal(elapsed, 2000);
});

test('covered, disabled, hidden and replaced targets never yield a click point', () => {
    for (const [option, code] of [['obscured','element_obscured'], ['disabled','element_disabled'], ['hidden','element_not_visible'], ['stale','stale_reference']]) {
        assert.equal(probe({[option]:true}).result.error.code, code);
    }
});
