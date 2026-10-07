# Live views

A page whose state lives on the server: browser events run a naga word, and the page
re-renders, LiveView-style. Under `naga start` it updates as you edit the word.

## A live page is one word

```
live /counter = counter

counter {-event: {name: :inc}} = count+1 render
counter {-event: {name: :dec}} = count-1 render
counter {!count}               = +{count: 0} render
counter                        = render

render = +{html: [:div, [:h1, "Count: {count}"],
                  [:button, {on-click: :inc}, "+"]]}
```

- `live PATH = word` is declared like a route, with path and query params but no verb or body.
- The word runs on the session's record:
  - **on mount**, the record is the params (and `time`);
  - **on an event**, it's the state plus `event: {name: :inc, …}`.
- The word leaves `html`. Everything else it leaves, apart from `event`, is the next state.

## Markup is naga values

- An element is `[:tag, {attrs}?, child…]`.
- Strings are escaped.
- Nested lists are flattened, so `@rows( … )` can build a list of elements.
- An attribute that is `nil` or `false` is left out; one that is `true` is written bare.
- Events: `on-click: :inc` sends `event: {name: :inc}`. `on-submit` adds `form: {…}`, and
  `on-input` adds `value` (slice 2).

## Runtime

```
GET /counter ──▶ mount: run the word on the params → full page (html + client + session id)
ws /naga/live?s=id ◀──▶ events in, html out; the client morphs the DOM (morphdom, bundled)
```

- **Sessions** live in a shared map, each behind its own lock: one session's events stay in
  order, different sessions run in parallel. A session goes when its socket closes, or a
  minute after mount if none connects.
- **Everyone stays current (slice 2):**
  - an event or request that writes to a table re-renders every session and pushes the
    ones whose HTML changed;
  - each new image the editor publishes re-renders every session with the new word,
    keeping its state.
- **Failures stay in the session (slice 3):** an event that errors shows an error box in that
  page.

## Checker (slice 3)

A live page is a call site like a route: its input is the params, its state is open, and
the word must leave `html`.

## Slices

1. `live` declarations, markup to HTML, mount, the websocket, the bundled client, click
   events; a counter example end to end.
2. Form and input events, re-rendering on writes and on new images, dropping sessions.
3. The checker, error boxes, runs (event scripts with expected state), and stepping a live
   event in the debugger.
