const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../../crates/horizon-browser/src/webauthn/page.js'), 'utf8')
    .replace('__DELIVER_KEY__', '"__deliver"')
    .replace('__MAX_MESSAGE_BYTES__', '4096');

// Install the page script in a fresh realm and record what it posts.
function install() {
    const posted = [];
    class CredentialsContainer {
        get() { return Promise.resolve('native get'); }
        create() { return Promise.resolve('native create'); }
    }
    class PublicKeyCredential {
        static isConditionalMediationAvailable() { return Promise.resolve(true); }
        static isUserVerifyingPlatformAuthenticatorAvailable() { return Promise.resolve(false); }
    }
    class AuthenticatorAssertionResponse {}
    class AuthenticatorAttestationResponse {}
    const context = vm.createContext({
        CredentialsContainer, PublicKeyCredential, AuthenticatorAssertionResponse, AuthenticatorAttestationResponse,
        DOMException, TextEncoder, AbortController, btoa, atob,
    });
    context.navigator = {credentials: new CredentialsContainer()};
    vm.runInContext(`(${source})`, context)((message) => posted.push(JSON.parse(message)));
    return {context, posted, credentials: context.navigator.credentials};
}

const challenge = new Uint8Array([1, 2, 3, 4]);
const user = {id: new Uint8Array([9]), name: 'u', displayName: 'U'};
const pubKeyCredParams = [{type: 'public-key', alg: -7}];

test('an unknown residentKey counts as absent so requireResidentKey applies', () => {
    const {credentials, posted} = install();
    credentials.create({publicKey: {challenge, rp: {name: 'x'}, user, pubKeyCredParams,
        authenticatorSelection: {residentKey: 'unknown-value', requireResidentKey: true}}});
    credentials.create({publicKey: {challenge, rp: {name: 'x'}, user, pubKeyCredParams,
        authenticatorSelection: {residentKey: 'preferred', requireResidentKey: true}}});
    assert.equal(posted[0].options.resident_key, 'required');
    assert.equal(posted[1].options.resident_key, 'preferred');
});

test('conditional mediation never posts and settles on abort', async () => {
    const {credentials, posted, context} = install();
    assert.equal(await context.PublicKeyCredential.isConditionalMediationAvailable(), false);
    for (const op of ['get', 'create']) {
        const controller = new AbortController();
        const pending = credentials[op]({mediation: 'conditional', signal: controller.signal,
            publicKey: {challenge, rp: {name: 'x'}, user, pubKeyCredParams}});
        controller.abort();
        await assert.rejects(pending, {name: 'AbortError'});
    }
    assert.equal(posted.length, 0);
});

test('an options getter that aborts the signal never posts', async () => {
    const {credentials, posted} = install();
    const controller = new AbortController();
    const pending = credentials.get({signal: controller.signal, publicKey: {
        get challenge() { controller.abort(); return challenge; },
    }});
    await assert.rejects(pending, {name: 'AbortError'});
    assert.equal(posted.length, 0);
});

test('an abort after posting sends an abort message and rejects', async () => {
    const {credentials, posted} = install();
    const controller = new AbortController();
    const pending = credentials.get({signal: controller.signal, publicKey: {challenge}});
    controller.abort();
    await assert.rejects(pending, {name: 'AbortError'});
    assert.deepEqual(posted.map((message) => message.kind), ['request', 'abort']);
    assert.equal(posted[1].id, posted[0].id);
});

test('options are bounded before they are posted', async () => {
    const {credentials, posted} = install();
    credentials.get({publicKey: {challenge, timeout: 1e21}});
    assert.equal(posted[0].options.timeout, 0xffffffff);
    await assert.rejects(credentials.create({publicKey: {challenge, rp: {name: 'x'}, user,
        pubKeyCredParams: [{type: 'public-key'}]}}), {name: 'TypeError'});
    await assert.rejects(credentials.get({publicKey: {challenge: new Uint8Array(8192)}}), {name: 'TypeError'});
    await assert.rejects(credentials.get({publicKey: {challenge: new Uint8Array(0)}}), {name: 'TypeError'});
    assert.equal(posted.length, 1);
});

test('a create without a required member is a TypeError and never posts', async () => {
    const {credentials, posted} = install();
    const complete = {challenge, rp: {name: 'x'}, user, pubKeyCredParams};
    for (const publicKey of [
        {...complete, rp: {}},
        {...complete, user: {id: user.id, displayName: 'U'}},
        {...complete, user: {id: user.id, name: 'u'}},
        {challenge, rp: {name: 'x'}, user},
    ]) {
        await assert.rejects(credentials.create({publicKey}), {name: 'TypeError'});
    }
    assert.equal(posted.length, 0);
    await assert.rejects(credentials.create({publicKey: {...complete, pubKeyCredParams: [{type: 'foo', alg: -7}]}}),
        {name: 'NotSupportedError'});
    assert.equal(posted.length, 0);
    credentials.create({publicKey: {...complete, pubKeyCredParams: []}});
    assert.deepEqual(posted[0].options.algorithms, []);
});

test('other credential types keep the native API', async () => {
    const {credentials, posted} = install();
    assert.equal(await credentials.get({password: true}), 'native get');
    assert.equal(await credentials.create({password: {}}), 'native create');
    assert.equal(posted.length, 0);
});

test('a reply resolves a credential with the page JSON shape', async () => {
    const {credentials, posted, context} = install();
    const pending = credentials.get({publicKey: {challenge}});
    context.__deliver(JSON.stringify({id: posted[0].id, credential: {
        credential_id: 'AQID', client_data_json: 'e30', authenticator_data: 'AAAA', signature: 'BBBB', user_handle: null,
    }}));
    const credential = await pending;
    assert.ok(credential instanceof context.PublicKeyCredential);
    assert.equal(credential.id, 'AQID');
    assert.equal(credential.authenticatorAttachment, 'platform');
    assert.equal(credential.response.userHandle, null);
    const json = credential.toJSON();
    assert.deepEqual(Object.keys(json.response).sort(), ['authenticatorData', 'clientDataJSON', 'signature']);
});

test('an error reply rejects with the named DOMException', async () => {
    const {credentials, posted, context} = install();
    const pending = credentials.create({publicKey: {challenge, rp: {name: 'x'}, user, pubKeyCredParams}});
    context.__deliver(JSON.stringify({id: posted[0].id, error: {name: 'InvalidStateError', message: 'excluded'}}));
    await assert.rejects(pending, {name: 'InvalidStateError'});
});
