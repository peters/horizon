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
    const algorithmOf = (parameter) => {
        const alg = Number(parameter.alg);
        if (!Number.isInteger(alg)) throw new TypeError("pubKeyCredParams[].alg must be an integer");
        return alg;
    };
    const credentialIds = (list) =>
        Array.from(list || [])
            .filter((entry) => entry && entry.type === "public-key")
            .map((entry) => encode(entry.id));
    const failure = (name, message) =>
        name === "TypeError" ? new TypeError(message) : new DOMException(message || NOT_ALLOWED, name || "NotAllowedError");

    const getOptions = (publicKey) => ({
        challenge: challengeOf(publicKey.challenge),
        rp_id: text(publicKey.rpId),
        allow_credentials: credentialIds(publicKey.allowCredentials),
        user_verification: text(publicKey.userVerification),
        timeout: timeoutOf(publicKey.timeout),
    });
    const createOptions = (publicKey) => {
        const rp = publicKey.rp;
        const user = publicKey.user;
        // The required dictionary members of PublicKeyCredentialCreationOptions,
        // as WebIDL checks them in a browser before any authenticator runs.
        const missing = !rp || rp.name === undefined || !user || user.name === undefined
            || user.displayName === undefined || publicKey.pubKeyCredParams === undefined;
        if (missing) throw new TypeError("publicKey is missing a required member");
        const selection = publicKey.authenticatorSelection || {};
        const parameters = Array.from(publicKey.pubKeyCredParams);
        const algorithms = parameters.filter((parameter) => parameter && parameter.type === "public-key").map(algorithmOf);
        // An empty list asks for the defaults; a list of only unknown types
        // offers nothing that a public-key credential can use.
        if (parameters.length && !algorithms.length) {
            throw new DOMException("No supported public key algorithm", "NotSupportedError");
        }
        return {
            challenge: challengeOf(publicKey.challenge),
            rp_id: text(rp.id),
            rp_name: text(rp.name),
            user: { id: encode(user.id), name: text(user.name), display_name: text(user.displayName) },
            algorithms,
            exclude_credentials: credentialIds(publicKey.excludeCredentials),
            // An unknown residentKey counts as absent, so requireResidentKey applies.
            resident_key: residentKeys.includes(selection.residentKey)
                ? selection.residentKey
                : selection.requireResidentKey ? "required" : undefined,
            user_verification: text(selection.userVerification),
            timeout: timeoutOf(publicKey.timeout),
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
        try {
            const options = op === "get" ? getOptions(publicKey) : createOptions(publicKey);
            message = stringify({ kind: "request", id, op, options });
        } catch (error) {
            const known = error instanceof TypeError || error instanceof DOMException;
            return Promise.reject(known ? error : new TypeError(String(error)));
        }
        if (encoder.encode(message).length > maxMessageBytes) {
            return Promise.reject(new TypeError("The publicKey options are too large"));
        }
        // An options getter can abort the signal; check it after reading them.
        if (signal?.aborted) return Promise.reject(signal.reason);
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
    replace(Container.prototype, "get", function get(options) {
        if (!options || typeof options !== "object" || !options.publicKey) return nativeGet.apply(this, arguments);
        if (options.mediation === "conditional") return conditional(options.signal);
        return request("get", options.publicKey, options.signal);
    });
    replace(Container.prototype, "create", function create(options) {
        if (!options || typeof options !== "object" || !options.publicKey) return nativeCreate.apply(this, arguments);
        if (options.mediation === "conditional") return conditional(options.signal);
        return request("create", options.publicKey, options.signal);
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
