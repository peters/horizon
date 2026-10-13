(send) => {
    "use strict";
    const deliverKey = __DELIVER_KEY__;
    const maxMessageBytes = __MAX_MESSAGE_BYTES__;
    const existing = Object.getOwnPropertyDescriptor(globalThis, deliverKey);
    if (existing) {
        if (typeof send === "function" && typeof existing.value?.rebind === "function") existing.value.rebind(send);
        return;
    }
    if (typeof send !== "function") return;
    const Container = globalThis.CredentialsContainer;
    if (typeof Container !== "function" || !globalThis.navigator?.credentials) return;

    const { stringify, parse } = JSON;
    const nativeGet = Container.prototype.get;
    const nativeCreate = Container.prototype.create;
    const toStringTag = Object.prototype.toString;
    const encoder = new TextEncoder();
    const NOT_ALLOWED = "The operation either timed out or was not allowed.";
    const pending = new Map();
    let post = send;
    let nextId = 0;

    const bytesOf = (source) => {
        if (ArrayBuffer.isView(source)) return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
        if (source instanceof ArrayBuffer || toStringTag.call(source) === "[object ArrayBuffer]") return new Uint8Array(source);
        throw new TypeError("Expected a BufferSource");
    };
    const encode = (source) => {
        const bytes = bytesOf(source);
        let binary = "";
        for (let offset = 0; offset < bytes.length; offset += 0x8000) {
            binary += String.fromCharCode.apply(null, bytes.subarray(offset, offset + 0x8000));
        }
        return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    };
    const decode = (text) => {
        const binary = atob(String(text).replace(/-/g, "+").replace(/_/g, "/"));
        const bytes = new Uint8Array(binary.length);
        for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
        return bytes.buffer;
    };
    const residentKeys = ["discouraged", "preferred", "required"];
    const challengeOf = (source) => {
        const encoded = encode(source);
        if (!encoded) throw new TypeError("publicKey.challenge must not be empty");
        return encoded;
    };
    const text = (value) => (value === undefined || value === null ? undefined : String(value));
    const timeoutOf = (value) =>
        Number.isFinite(value) && value >= 0 ? Math.min(Math.floor(value), 0xffffffff) : undefined;
    // `alg` is a required WebIDL `long`: missing is a TypeError, and a value
    // converts like ToInt32 (`1.5` is 1, `"abc"` is 0).
    const algorithmOf = (parameter) => {
        const alg = parameter.alg;
        if (alg === undefined) throw new TypeError("pubKeyCredParams[].alg is required");
        return Number(alg) | 0;
    };
    // Converts every descriptor, as WebIDL does, so a bad id is a TypeError
    // even in an entry of another type; then keeps the public-key ones.
    // A WebIDL sequence: `undefined` is an omitted member; anything that is
    // not an iterable object (`null` and strings included) is a TypeError.
    const sequenceOf = (value, name) => {
        if (value === undefined) return [];
        if (value === null || typeof value !== "object" || typeof value[Symbol.iterator] !== "function") {
            throw new TypeError(`${name} must be a sequence`);
        }
        return Array.from(value);
    };
    // A dictionary entry with a required `type` member.
    const typeOf = (entry, name) => {
        if (entry === null || typeof entry !== "object" || entry.type === undefined) {
            throw new TypeError(`${name} entries need a type`);
        }
        return String(entry.type);
    };
    const credentialIds = (list, name) => {
        const descriptors = sequenceOf(list, name);
        const ids = descriptors.map((entry) => ({ type: typeOf(entry, name), id: encode(entry.id) }));
        return { offered: descriptors.length, ids: ids.filter((entry) => entry.type === "public-key").map((entry) => entry.id) };
    };
    const failure = (name, message) =>
        name === "TypeError" ? new TypeError(message) : new DOMException(message || NOT_ALLOWED, name || "NotAllowedError");

    // Each builder throws TypeError for malformed options and returns a
    // `refusal` for valid options that no platform credential can satisfy.
    // The caller checks the abort signal between the two.
    const getOptions = (publicKey) => {
        const challenge = challengeOf(publicKey.challenge);
        const allowed = credentialIds(publicKey.allowCredentials, "allowCredentials");
        return {
            options: {
                challenge,
                rp_id: text(publicKey.rpId),
                allow_credentials: allowed.ids,
                user_verification: text(publicKey.userVerification),
                timeout: timeoutOf(publicKey.timeout),
            },
            // Entries but none of type public-key allow no credential; an
            // empty list would mean "any discoverable credential".
            refusal: allowed.offered && !allowed.ids.length ? new DOMException(NOT_ALLOWED, "NotAllowedError") : null,
        };
    };
    const createOptions = (publicKey) => {
        const rp = publicKey.rp;
        const user = publicKey.user;
        // The required dictionary members of PublicKeyCredentialCreationOptions,
        // as WebIDL checks them in a browser before any authenticator runs.
        const missing = !rp || rp.name === undefined || !user || user.name === undefined
            || user.displayName === undefined || publicKey.pubKeyCredParams === undefined;
        if (missing) throw new TypeError("publicKey is missing a required member");
        const selection = publicKey.authenticatorSelection || {};
        const challenge = challengeOf(publicKey.challenge);
        const userId = encode(user.id);
        const parameters = sequenceOf(publicKey.pubKeyCredParams, "pubKeyCredParams");
        // Every entry is converted, so a missing or bad alg is a TypeError
        // whatever its type; then only public-key entries are kept.
        const algorithms = parameters
            .map((parameter) => ({ type: typeOf(parameter, "pubKeyCredParams"), alg: algorithmOf(parameter) }))
            .filter((parameter) => parameter.type === "public-key")
            .map((parameter) => parameter.alg);
        const excluded = credentialIds(publicKey.excludeCredentials, "excludeCredentials");
        let refusal = null;
        if (parameters.length && !algorithms.length) {
            // An empty list asks for the defaults; a list of only unknown
            // types offers nothing that a public-key credential can use.
            refusal = new DOMException("No supported public key algorithm", "NotSupportedError");
        } else if (selection.authenticatorAttachment === "cross-platform") {
            // The command answers as a platform authenticator.
            refusal = new DOMException(NOT_ALLOWED, "NotAllowedError");
        }
        return {
            options: {
                challenge,
                rp_id: text(rp.id),
                rp_name: text(rp.name),
                user: { id: userId, name: text(user.name), display_name: text(user.displayName) },
                algorithms,
                exclude_credentials: excluded.ids,
                // An unknown residentKey counts as absent, so requireResidentKey applies.
                resident_key: residentKeys.includes(selection.residentKey)
                    ? selection.residentKey
                    : selection.requireResidentKey ? "required" : undefined,
                user_verification: text(selection.userVerification),
                timeout: timeoutOf(publicKey.timeout),
            },
            refusal,
        };
    };

    const prototypeOf = (name) => globalThis[name]?.prototype || Object.prototype;
    const value = (inner) => ({ value: inner, enumerable: true });
    const credential = (op, result) => {
        const id = String(result.credential_id);
        let response;
        let responseJson;
        if (op === "get") {
            response = Object.create(prototypeOf("AuthenticatorAssertionResponse"), {
                clientDataJSON: value(decode(result.client_data_json)),
                authenticatorData: value(decode(result.authenticator_data)),
                signature: value(decode(result.signature)),
                userHandle: value(result.user_handle ? decode(result.user_handle) : null),
            });
            responseJson = {
                clientDataJSON: result.client_data_json,
                authenticatorData: result.authenticator_data,
                signature: result.signature,
            };
            if (result.user_handle) responseJson.userHandle = result.user_handle;
        } else {
            const transports = Array.from(result.transports || [], String);
            response = Object.create(prototypeOf("AuthenticatorAttestationResponse"), {
                clientDataJSON: value(decode(result.client_data_json)),
                attestationObject: value(decode(result.attestation_object)),
                getAuthenticatorData: value(() => decode(result.authenticator_data)),
                getPublicKey: value(() => (result.public_key ? decode(result.public_key) : null)),
                getPublicKeyAlgorithm: value(() => result.public_key_algorithm),
                getTransports: value(() => transports.slice()),
            });
            responseJson = {
                clientDataJSON: result.client_data_json,
                authenticatorData: result.authenticator_data,
                transports: transports.slice(),
                publicKeyAlgorithm: result.public_key_algorithm,
                attestationObject: result.attestation_object,
            };
            if (result.public_key) responseJson.publicKey = result.public_key;
        }
        return Object.create(prototypeOf("PublicKeyCredential"), {
            id: value(id),
            rawId: value(decode(id)),
            type: value("public-key"),
            response: value(response),
            authenticatorAttachment: value("platform"),
            getClientExtensionResults: value(() => ({})),
            toJSON: value(() => ({
                id,
                rawId: id,
                type: "public-key",
                authenticatorAttachment: "platform",
                clientExtensionResults: {},
                response: { ...responseJson },
            })),
        });
    };

    const request = (op, publicKey, signal) => {
        nextId += 1;
        const id = nextId;
        let message;
        let refusal;
        try {
            const built = op === "get" ? getOptions(publicKey) : createOptions(publicKey);
            refusal = built.refusal;
            message = stringify({ kind: "request", id, op, options: built.options });
        } catch (error) {
            // A builder throws TypeError; an exception from a page getter
            // propagates unchanged, as WebIDL does.
            return Promise.reject(error);
        }
        if (encoder.encode(message).length > maxMessageBytes) {
            return Promise.reject(new TypeError("The publicKey options are too large"));
        }
        // An options getter can abort the signal; check it after reading them,
        // and before a refusal, as a browser does.
        if (signal?.aborted) return Promise.reject(signal.reason);
        if (refusal) return Promise.reject(refusal);
        return new Promise((resolve, reject) => {
            const onAbort = () => {
                if (!pending.delete(id)) return;
                try { post(stringify({ kind: "abort", id })); } catch (_) {}
                reject(signal.reason);
            };
            const cleanup = () => signal?.removeEventListener("abort", onAbort);
            pending.set(id, { op, resolve, reject, cleanup });
            signal?.addEventListener("abort", onAbort, { once: true });
            try {
                post(message);
            } catch (_) {
                pending.delete(id);
                cleanup();
                reject(failure("NotAllowedError", NOT_ALLOWED));
            }
        });
    };
    // Conditional mediation never reaches the authenticator command: the
    // request stays pending until the page aborts it.
    const conditional = (signal) =>
        new Promise((_, reject) => {
            if (signal?.aborted) reject(signal.reason);
            else signal?.addEventListener("abort", () => reject(signal.reason), { once: true });
        });

    const deliver = (raw) => {
        let reply;
        try { reply = parse(raw); } catch (_) { return; }
        const entry = reply && pending.get(reply.id);
        if (!entry) return;
        pending.delete(reply.id);
        entry.cleanup();
        if (reply.error) {
            entry.reject(failure(reply.error.name, reply.error.message));
            return;
        }
        try {
            entry.resolve(credential(entry.op, reply.credential));
        } catch (_) {
            entry.reject(failure("NotAllowedError", NOT_ALLOWED));
        }
    };
    deliver.rebind = (next) => { post = next; };
    Object.defineProperty(globalThis, deliverKey, { value: deliver });

    const replace = (target, name, implementation) => {
        const descriptor = Object.getOwnPropertyDescriptor(target, name);
        if (!descriptor) return;
        Object.defineProperty(target, name, { ...descriptor, value: { [name]: implementation }[name] });
    };
    // Reads each member once; an exception from a page getter becomes a
    // rejected promise, as for a native promise-returning method.
    const route = (op, native, receiver, args) => {
        const options = args[0];
        let publicKey;
        let mediation;
        let signal;
        try {
            if (!options || typeof options !== "object") return native.apply(receiver, args);
            publicKey = options.publicKey;
            if (!publicKey) return native.apply(receiver, args);
            mediation = options.mediation;
            signal = options.signal;
        } catch (error) {
            return Promise.reject(error);
        }
        if (mediation === "conditional") return conditional(signal);
        return request(op, publicKey, signal);
    };
    replace(Container.prototype, "get", function get() {
        return route("get", nativeGet, this, arguments);
    });
    replace(Container.prototype, "create", function create() {
        return route("create", nativeCreate, this, arguments);
    });
    const Credential = globalThis.PublicKeyCredential;
    if (typeof Credential === "function") {
        const resolved = (answer) => () => Promise.resolve(answer);
        replace(Credential, "isConditionalMediationAvailable", resolved(false));
        replace(Credential, "isUserVerifyingPlatformAuthenticatorAvailable", resolved(true));
        replace(Credential, "getClientCapabilities", () => Promise.resolve({
            conditionalCreate: false,
            conditionalGet: false,
            hybridTransport: false,
            passkeyPlatformAuthenticator: true,
            userVerifyingPlatformAuthenticator: true,
        }));
    }
}
