const done = arguments[arguments.length - 1];
const pending = window.__horizonPendingPromise;
try {
  delete window.__horizonPendingPromise;
} catch (_) {
  // The answer is below either way.
}
if (pending === undefined) {
  done({horizonLost: true});
} else {
  Promise.resolve(pending).then(
    (value) => done({horizonSettled: true, horizonValue: value === undefined ? null : value}),
    (error) => {
      let reason;
      try {
        reason = String(error && error.message ? error.message : error);
      } catch (_) {
        reason = 'an unprintable value';
      }
      done({horizonRejected: reason});
    }
  );
}
