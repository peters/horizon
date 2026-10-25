---
applyTo: "crates/horizon-browser/src/semantic.rs,crates/horizon-browser/src/semantic_keys.rs,crates/horizon-browser/src/session/semantic.rs,crates/horizon-browser/src/session/semantic/**,crates/horizon-browser/src/webdriver/session/semantic.rs,crates/horizon-browser/src/webdriver/session/semantic/**"
---

# Trust boundary of browser semantic actions

A semantic action turns an agent request into one reference, selector, key or
text and reads the page back to judge the result. The design is in
`crates/horizon-browser-mcp/README.md` and in the module comments of
`crates/horizon-browser/src/semantic.rs`.

## Trusted: the agent request

The reference, selector, key chord, text and frame route of an action come from
the agent through the MCP server.

- A short-lived reference resolves to one selector and one recorded child-frame
  route. An unknown or expired reference is a `stale_reference`.
- A reference that carries a frame route is used only after it is checked
  against the current frame contexts of this panel.
- The text of a fill is never written to the action audit. The audit names
  printable key presses, not their characters.
- The text is compared with the page only. Its result is one boolean, never the
  page's current value.

## Untrusted: the page document

The page reports its rectangles, its hit test, its focus and the value it kept.
The frame reports its own positions.

- The selector and the fill text are quoted as data in every expression that is
  evaluated in the page.
- Every page observation is a bounded value of fixed shape, for example one
  number, one boolean or one element name from a closed set. Page text is not
  returned to the agent and not logged.
- A count of page text inside an error is the page's own error message, cut to
  512 bytes, as the page scripts already did.
- A frame route is host-owned. A page cannot return or change a route.
- A click point is measured and hit-tested inside the frame, in the coordinate
  space that the session of that frame dispatches in. A point that another
  element owns or that is outside the frame viewport is a typed failure, not a
  completion.
- A frame that does not take focus after a click that names a focusable target
  is a typed failure.
- A fill that the page did not keep is a typed failure.

## Does not guarantee

- The page can refuse a click that reached it, for example with its own state
  machine, and the action still reports completion. The agent examines the
  visible result.
- The page can move focus after it takes a click. That is reported as
  completion.
- A page can make an action fail by covering its own control or by moving
  focus. A failure is not proof that the page is hostile.
- A same-process child shares the page's focus, so its focus cannot prove that
  one click arrived.
- The readback of a fill is an immediate postcondition. A page can change the
  value later.
