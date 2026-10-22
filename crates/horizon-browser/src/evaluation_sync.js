try {
  Object.defineProperty(window, '__HORIZON_GLOBAL__', {
    value: undefined,
    configurable: true,
    writable: true,
  });
} catch (_) {
  return {horizonUnstorable: true};
}
try {
  window.__HORIZON_GLOBAL__ = (__HORIZON_EXPRESSION__);
} catch (horizonExpressionFailure) {
  delete window.__HORIZON_GLOBAL__;
  throw horizonExpressionFailure;
}
const horizonKept = window.__HORIZON_GLOBAL__;
delete window.__HORIZON_GLOBAL__;
if (horizonKept && typeof horizonKept.then === 'function') {
  window.__HORIZON_GLOBAL__ = horizonKept;
  return {horizonPending: true};
}
return {horizonSettled: true, horizonValue: horizonKept === undefined ? null : horizonKept};
