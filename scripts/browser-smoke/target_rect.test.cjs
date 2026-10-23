const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const {MessageChannel} = require('node:worker_threads');
const source = fs.readFileSync(path.join(__dirname, '../../crates/horizon-browser/src/semantic.rs'), 'utf8');
const script = source.match(/const TARGET_RECT_FUNCTION: &str = r"([\s\S]*?)";/)[1];

function probe(options = {}) {
    let cleared = false;
    const document = {activeElement: null};
    const element = {
        tagName: 'INPUT',
        value: 'before',
        matches: () => !!options.disabled,
        getAttribute: () => null,
        scrollIntoView: () => {},
        getBoundingClientRect: () => ({left:16, top:10, width:86.671875, height:36}),
        focus: () => { document.activeElement = element; },
        dispatchEvent: () => { cleared = true; if (options.disableAfterInput) options.disabled = true; },
    };
    document.querySelector = () => options.missing ? null : element;
    const context = {
        document,
        MessageChannel,
        Event: class {},
        HTMLInputElement: {prototype: {}},
        getComputedStyle: () => ({display:options.hidden ? 'none' : 'block', visibility:'visible'}),
    };
    const run = vm.runInNewContext('(' + script + ')', context);
    return {run, element, cleared: () => cleared};
}

test('click and remote fill geometry serializes immediately without awaiting a promise', () => {
    const fixture = probe();
    const result = fixture.run('#target', false);
    assert.equal(typeof result.then, 'undefined');
    assert.deepEqual(JSON.parse(JSON.stringify(result)), {x:59.3359375, y:28});
    assert.equal(fixture.element.value, 'before');
    assert.equal(fixture.cleared(), false);
});

test('actionability failures remain synchronously visible to classic drivers', () => {
    for (const [option, code] of [['missing','no_such_element'], ['hidden','element_not_visible'], ['disabled','element_disabled']]) {
        const result = probe({[option]:true}).run('#target', false);
        assert.equal(typeof result.then, 'undefined');
        assert.equal(JSON.parse(JSON.stringify(result)).error.code, code);
    }
});

test('local clearing still awaits focus and input handlers before returning geometry', async () => {
    const fixture = probe();
    const result = fixture.run('#target', true);
    assert.equal(typeof result.then, 'function');
    assert.equal(fixture.element.value, 'before');
    assert.deepEqual(JSON.parse(JSON.stringify(await result)), {x:59.3359375, y:28});
    assert.equal(fixture.element.value, '');
    assert.equal(fixture.cleared(), true);
});

test('an input handler that disables the field still prevents a subsequent fill', async () => {
    const fixture = probe({disableAfterInput:true});
    const result = await fixture.run('#target', true);
    assert.equal(result.error.code, 'element_disabled');
});
