// Node test for #574 (follow-up from #554's review 3): an embed-mode
// ::IMAGE fetch's blob: URL must not leak when its OWN run never reaches
// its own swap-time sweep (web/app.js's runOnce, see dpImgRunBlobs' own
// comment) — e.g. the run is superseded before getting there. Extracts the
// pure reap logic, reapOtherRunBlobs, and exercises it with plain mock
// `img`-like objects (only `.isConnected` / `.dataset.anofoxBlob` matter —
// no real DOM needed). The DOM-wiring (mkImg's fetch, the per-run
// candidateBlobImgs sweep around it) has no standalone pure form this
// file's regex-extract convention can isolate, the same reasoning
// tests/web/views_test.mjs gives for its own DOM-manipulating cases; that
// part is covered by scripts/auth-e2e/run.sh's embed/image-first case
// (real browser) and by manual trace during implementation.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { reapOtherRunBlobs } = new Function(`${grab("reapOtherRunBlobs")}\nreturn { reapOtherRunBlobs };`)();

const img = (connected, blob) => ({ isConnected: connected, dataset: { anofoxBlob: blob } });

// A mock revoke() that just records what it was called with.
const spy = () => {
  const calls = [];
  const fn = (url) => calls.push(url);
  fn.calls = calls;
  return fn;
};

// ---- the gap #574 fixes: a run superseded before its OWN sweep ----
{
  // Run 7's own swap-time sweep never happened (its statement loop was cut
  // short, or it was otherwise abandoned) — its tracked image never got
  // revoked by its own code path. Run 8 is the next one to actually reach
  // a sweep.
  const runBlobs = new Map([[7, new Set([img(false, "blob:dead-7")])]]);
  const revoke = spy();
  const revoked = reapOtherRunBlobs(runBlobs, 8, revoke);
  assert.deepEqual(revoke.calls, ["blob:dead-7"], "run 8's sweep revokes run 7's orphaned blob");
  assert.equal(revoked.length, 1);
  assert.equal(runBlobs.has(7), false, "run 7's now-empty entry is pruned, not left to grow forever");
}

// ---- a connected image from an older, abandoned run is left alone (still
// in use — the live-DOM sweep around the call site, not this function, now
// owns its eventual cleanup) ----
{
  const keptImg = img(true, "blob:kept-3");
  const runBlobs = new Map([[3, new Set([keptImg])]]);
  const revoke = spy();
  reapOtherRunBlobs(runBlobs, 9, revoke);
  assert.deepEqual(revoke.calls, [], "a connected image is never revoked");
  assert.equal(runBlobs.has(3), false, "it's dropped from tracking either way — the DOM sweep owns it now");
}

// ---- the calling run's OWN entry is dropped untouched, never revoked
// here — its OWN candidateBlobImgs sweep (the caller, around this
// function) already resolved every one of its images before calling this ----
{
  const ownImg = img(false, "blob:own-5"); // even a disconnected one of its OWN
  const runBlobs = new Map([[5, new Set([ownImg])]]);
  const revoke = spy();
  reapOtherRunBlobs(runBlobs, 5, revoke);
  assert.deepEqual(revoke.calls, [], "the calling run's own entry is just dropped, never revoked by this function");
  assert.equal(runBlobs.size, 0);
}

// ---- several abandoned runs at once; mixed connected/disconnected within
// one run; an image with no anofoxBlob set yet (dataset not populated) is
// skipped, not revoke(undefined) ----
{
  const runBlobs = new Map([
    [1, new Set([img(false, "blob:a"), img(true, "blob:b")])],
    [2, new Set([img(false, "blob:c"), img(false, undefined)])],
  ]);
  const revoke = spy();
  const revoked = reapOtherRunBlobs(runBlobs, 99, revoke);
  assert.deepEqual(new Set(revoke.calls), new Set(["blob:a", "blob:c"]), `${revoke.calls}`);
  assert.equal(revoked.length, 2);
  assert.equal(runBlobs.size, 0, "every run's set is pruned empty");
}

console.log("embed_image_blob_test: ok");
