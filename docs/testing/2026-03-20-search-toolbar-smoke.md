# Search Toolbar Smoke Test

## Scope

Validate the inline terminal-search field after the toolbar chrome refresh.
Focus on visual treatment, text entry, dropdown anchoring, resize behavior, and
result navigation staying intact.

## Environment

1. Build and run Horizon from the current checkout.
2. Use a temporary `HOME` so the smoke pass does not mutate the main session.
3. Seed at least three visible terminal panels across one or more workspaces.
4. Put distinct searchable text in each panel so multi-panel matches are easy to
   confirm.
5. Leave enough empty canvas visible that toolbar contrast and alignment can be
   judged clearly.

## Baseline

1. Launch Horizon and confirm the first frame renders without missing toolbar
   chrome.
2. Confirm the toolbar search field is visible at the top of the workspace.
3. Confirm the field shows the placeholder text when empty.
4. Confirm the new chrome reads clearly against the toolbar background:
   border, fill, icon badge, and hint text should all be visible without harsh
   clipping or overlap.

## Primary Flow

1. Click inside the search field and confirm the focus state becomes visually
   stronger than the idle state.
2. Type a query that matches text in more than one panel.
3. Confirm the dropdown opens beneath the field and stays visually aligned with
   the input.
4. Confirm the case and regex toggles remain clickable.
5. Confirm the result status line still reports total matches and panel count.
6. Use arrow keys to move selection and confirm the highlighted row updates.
7. Press Enter and confirm Horizon focuses the matched panel and scrolls the
   result into view.
8. Reopen the search field, enter a query with no matches, and confirm the
   empty state still renders cleanly.

## Edge Cases

1. Hover the field without focusing it and confirm the hover state is distinct
   but subtler than focus.
2. Clear the query and confirm the empty-field styling returns cleanly.
3. Enter a long query and confirm text does not overlap the left icon badge or
   clip against the right edge.
4. Click outside the field and confirm the dropdown dismisses normally.
5. Re-enter the field after dismissal and confirm focus, cursor, and dropdown
   behavior still work.

## Resize And Reset View

1. Capture a screenshot immediately after launch with the empty search field.
2. Resize the main window wider and narrower and confirm the field stretches
   cleanly without distorted corners, clipped placeholder text, or misplaced
   icon chrome.
3. Open the search dropdown during the resized state and confirm the dropdown
   still anchors to the input.
4. Trigger `Reset View` and confirm the toolbar search field remains visually
   stable after the fit/reset path.
5. Capture a second screenshot after the resize/reset interaction.

## Persistence

1. Close Horizon while the field is empty, relaunch, and confirm the search bar
   still renders in the refreshed style.
2. Repeat after performing at least one successful search to confirm the search
   UI still initializes cleanly on the next launch.

## Visual Regression Checks

1. Compare the launch and post-resize screenshots.
2. Confirm there is no text/icon overlap, no doubled borders, no clipped glow,
   and no fill leaking outside the rounded corners.
3. Confirm the refreshed field still matches Horizon's existing dark chrome and
   accent palette rather than appearing as a default platform input.
