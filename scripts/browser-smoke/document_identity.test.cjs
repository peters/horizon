const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');

function rustScript(file, name) {
    const source = fs.readFileSync(path.join(__dirname, '../../crates/horizon-browser/src', file), 'utf8');
    const match = source.match(new RegExp('const ' + name + ': &str = r"([\\s\\S]*?)";'));
    assert.ok(match, 'production script must be available');
    return match[1];
}

const identity = rustScript('document_identity.rs', 'DOCUMENT_IDENTITY_FUNCTION');
const scan = rustScript('semantic.rs', 'NODE_SCAN_FUNCTION');

function page() {
    let sequence = 0;
    const context = vm.createContext({
        document: {querySelectorAll: () => []},
        location: {href: 'https://example.test/lab'},
        performance: {timeOrigin: 1234},
        crypto: {getRandomValues: words => words.fill(++sequence)},
    });
    const read = () => vm.runInContext('(' + identity + ')()', context);
    const peek = () => vm.runInContext('(' + scan + ')(null, 10, true, false, (' + identity + ')())', context);
    return {context, read, peek};
}

test('clock drift and repeated standalone/scan executions preserve document identity', () => {
    const {context, read, peek} = page();
    const baseline = read();
    for (const timeOrigin of [1234.0002, 1233, 1222, 1235]) {
        context.performance.timeOrigin = timeOrigin;
        context.innerWidth = context.innerWidth === 820 ? 1180 : 820;
        context.innerHeight = context.innerWidth === 820 ? 1106 : 746;
        context.scrollY = timeOrigin;
        context.document.value = 'filled';
        assert.equal(read(), baseline);
        assert.equal(peek().documentIdentity, baseline);
    }
});

test('replacing the root in the same document preserves identity', () => {
    const {context, read} = page();
    const baseline = read();
    context.document.documentElement = {};
    assert.equal(read(), baseline);
    context.document.documentElement = {};
    assert.equal(read(), baseline);
});

test('same-URL document replacement invalidates even with identical clock values', () => {
    const {context, read, peek} = page();
    const baseline = read();
    context.document = {querySelectorAll: () => []};
    assert.notEqual(read(), baseline);
    assert.equal(read(), peek().documentIdentity);
});

test('URL changes retain existing identity invalidation semantics', () => {
    const {context, read} = page();
    const baseline = read();
    context.location.href += '#next';
    assert.notEqual(read(), baseline);
});

test('marker is an own immutable hidden property rather than DOM content', () => {
    const {context, read} = page();
    read();
    const keys = Object.getOwnPropertySymbols(context.document);
    assert.equal(keys.length, 1);
    const descriptor = Object.getOwnPropertyDescriptor(context.document, keys[0]);
    assert.equal(descriptor.writable, false);
    assert.equal(descriptor.configurable, false);
    assert.equal(descriptor.enumerable, false);
    assert.equal(typeof descriptor.value, 'string');
    assert.deepEqual(Object.keys(context.document), ['querySelectorAll']);
});

test('unusable document refuses instead of returning an unstable fallback identity', () => {
    const {context, read} = page();
    context.document = Object.preventExtensions({});
    assert.throws(read, /extensible/);
});
