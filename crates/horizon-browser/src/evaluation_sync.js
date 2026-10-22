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
if (window.__HORIZON_GLOBAL__ && typeof window.__HORIZON_GLOBAL__.then === 'function') {
  return {horizonPending: true};
}
return {
  horizonSettled: true,
  horizonValue: (function () {
    const horizonAnswer = window.__HORIZON_GLOBAL__;
    delete window.__HORIZON_GLOBAL__;
    return horizonAnswer === undefined ? null : horizonAnswer;
  })(),
};
