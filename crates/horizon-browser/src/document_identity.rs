pub(crate) const DOCUMENT_IDENTITY_FUNCTION: &str = r"function() {
    const key = Symbol.for('horizon.documentIdentity');
    if (!Object.prototype.hasOwnProperty.call(document, key)) {
        const words = globalThis.crypto.getRandomValues(new Uint32Array(4));
        const token = Array.from(words, word => word.toString(16)).join('-');
        Object.defineProperty(document, key, { value: token });
    }
    return JSON.stringify([String(location.href), document[key]]);
}";

pub(crate) fn document_identity_expression() -> String {
    // Browser privacy clocks can vary between reads of the same document.
    // A Document-owned marker survives layout changes and resets on navigation.
    format!("({DOCUMENT_IDENTITY_FUNCTION})()")
}
