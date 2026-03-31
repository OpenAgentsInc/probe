# ChatScreen Primary Shell And Setup Secondary Surface

## Summary

Issue `#36` makes the Probe TUI home screen explicitly chat-first.

The shell now defaults to:

- `Chat`
- `Setup`
- `Events`

instead of opening directly into the Apple FM setup surface.

## What Changed

The base screen is now the chat shell.

That means:

- the transcript-first view is the default Probe home surface
- Apple FM setup remains available, but as the `Setup` tab
- the `Events` tab remains a supporting inspection surface

This keeps setup reachable without letting it define the whole app.

## Why

The structural audit was explicit that Probe was still treating setup as the
app.

That was the wrong growth path.

Probe needs a home shell that can eventually host:

- the transcript
- the future bottom pane and composer
- overlays and approvals
- live tool/runtime turns

Moving setup into a secondary tab makes that future architecture possible
without losing the current setup inspection screen.

## Current Layout

The current tab roles are:

- `Chat`
  - primary home surface
  - retained transcript widget plus shell-side status and setup entry summary
- `Setup`
  - Apple FM setup details
  - backend facts and availability detail
- `Events`
  - app-shell and worker event logs

## Next

This issue intentionally stops before adding a real bottom interaction pane.

That is the next milestone:

- `#37` adds the `BottomPane` owner and minimal composer
