const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');

const source = path.join(__dirname, '../../crates/horizon-browser/src');
const syncTemplate = fs.readFileSync(path.join(source, 'evaluation_sync.js'), 'utf8');
const awaitScript = fs.readFileSync(path.join(source, 'evaluation_await.js'), 'utf8');
const placeholder = '__HORIZON_EXPRESSION__';

function plain(value) {
    return JSON.parse(JSON.stringify(value));
}
const globalName = '__horizonPendingPromise';

function page(extra = {}) {
    const window = {};
    const sandbox = {window, Promise, String, ...extra};
    return {
        window,
        window_global: () => window[globalName],
        evaluate(expression) {
            const script = syncTemplate.replace(placeholder, expression);
            const run = vm.runInNewContext(`(function () {${script}})`, sandbox);
            return run();
        },
        async settle(waited) {
            const run = vm.runInNewContext(`(function () {${awaitScript}})`, sandbox);
            let answer;
            await run((value) => {
                answer = value;
            });
            await waited;
            return answer;
        },
    };
}

test('a constant and an object answer directly and leave no page global', () => {
    const target = page();
    assert.deepEqual(plain(target.evaluate('1+1')), {horizonSettled: true, horizonValue: 2});
    assert.equal(target.window_global(), undefined);
    assert.deepEqual(plain(target.evaluate('({answer: 42})')), {horizonSettled: true, horizonValue: {answer: 42}});
    assert.equal(target.window_global(), undefined);
});

test('an undefined result is a null answer', () => {
    assert.deepEqual(plain(page().evaluate('undefined')), {horizonSettled: true, horizonValue: null});
});

test('a returned promise is kept, then settled once by the await script', async () => {
    const target = page();
    const answer = target.evaluate('Promise.resolve(42)');
    assert.equal(answer.horizonPending, true);
    assert.ok(target.window_global() instanceof Promise, 'the promise waits in the page');
    assert.deepEqual(plain(await target.settle()), {horizonSettled: true, horizonValue: 42});
    assert.equal(target.window_global(), undefined, 'the await removes the global');
});

test('the expression runs once, not once per call', async () => {
    let runs = 0;
    const target = page({
        count: () => {
            runs += 1;
            return runs;
        },
    });
    target.evaluate('count()');
    await target.settle();
    assert.equal(runs, 1);
});

test('a rejection carries its message and never a thrown reason', async () => {
    const target = page();
    target.evaluate("Promise.reject(new Error('synthetic rejection'))");
    assert.deepEqual(plain(await target.settle()), {horizonRejected: 'synthetic rejection'});
});

test('a rejection reason that cannot be printed is still reported', async () => {
    for (const expression of ['Promise.reject()', 'Promise.reject(null)', 'Promise.reject(Object.create(null))']) {
        const target = page();
        target.evaluate(expression);
        const answer = await target.settle();
        assert.equal(typeof answer.horizonRejected, 'string', expression);
        assert.ok(answer.horizonRejected.length > 0, expression);
    }
});

test('a page that pins the promise global fails typed instead of awaiting it', () => {
    const target = page();
    Object.defineProperty(target.window, globalName, {
        value: new Promise(() => {}),
        configurable: false,
        writable: false,
    });
    assert.deepEqual(plain(target.evaluate('Promise.resolve(5)')), {horizonUnstorable: true});
});

test('an earlier promise in the page cannot be awaited by a later call', async () => {
    const target = page();
    target.window[globalName] = Promise.resolve('from an earlier call');
    assert.deepEqual(plain(target.evaluate('1+1')), {horizonSettled: true, horizonValue: 2});
    assert.equal(target.window_global(), undefined, 'the evaluation cleared it first');
});

test('an await without the kept promise reports a lost document', async () => {
    assert.deepEqual(plain(await page().settle()), {horizonLost: true});
});

test('the scripts keep the page global the Rust side names', () => {
    const rust = fs.readFileSync(path.join(source, 'evaluation.rs'), 'utf8');
    const declared = rust.match(/const PENDING_PROMISE_GLOBAL: &str = "([^"]+)";/)[1];
    assert.equal(declared, globalName);
    assert.ok(syncTemplate.includes(globalName));
    assert.ok(awaitScript.includes(globalName));
    assert.ok(syncTemplate.includes(placeholder), 'the sync script keeps its expression placeholder');
});
