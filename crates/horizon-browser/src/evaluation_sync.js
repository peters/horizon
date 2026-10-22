try {
  delete window.__HORIZON_GLOBAL__;
} catch (_) {
  // A page cannot keep an older promise for this evaluation.
}
const value = (__HORIZON_EXPRESSION__);
if (value && typeof value.then === 'function') {
  try {
    Object.defineProperty(window, '__horizonPendingPromise', {
      value,
      configurable: true,
      writable: true,
    });
  } catch (_) {
    return {horizonUnstorable: true};
  }
  return {horizonPending: true};
}
return {horizonSettled: true, horizonValue: value === undefined ? null : value};
