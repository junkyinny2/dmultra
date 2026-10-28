# DM Ultra — Main Window UI Correction

## Objective

Modify the existing DM Ultra desktop application's main window to match the intended design described below. The current implementation is too sparse at the top, has an oversized Add URL area, uses incorrect spacing and proportions, and does not reproduce the compact, professional Internet Download Manager–style layout.

**Treat the existing application as the source of truth for functionality. This task is primarily a UI layout and styling correction, not a redesign or rewrite.** Preserve existing download logic, event handlers, navigation, commands, and application behavior.

## 1. Overall Window

Use a standard Windows desktop application window with:

- A light Windows title bar at the top.
- Application icon and text `DM Ultra` at the upper-left.
- Standard minimize, maximize/restore, and close buttons at the upper-right.
- A dark charcoal interface beneath the title bar.
- A thin, subtle border around the application window.
- A compact, information-dense layout that uses the available window area efficiently.

The reference window is approximately 909 × 614 pixels. These are reference proportions, not a fixed resolution. The UI must resize correctly and preserve its layout at different window sizes.

Use a consistent dark palette:
- Main background: approximately `#2D333F`.
- Toolbar background: approximately `#303744`.
- Sidebar background: approximately `#2D333F`.
- Dividers and borders: approximately `#414956`.
- Primary text: approximately `#D6D9E0`.
- Secondary text: approximately `#9299A5`.
- Accent blue: approximately `#3985F5`.
- Resume/active green: approximately `#21C45D`.

Avoid excessive borders, gradients, large empty header regions, and unnecessary decorative effects.

## 2. Menu Bar

Immediately below the title bar, create a compact horizontal menu bar approximately 27 pixels tall.

Left-aligned menu items, in this exact order:

`Tasks` | `File` | `Downloads` | `View` | `Help`

Use small, approximately 12–13 px text with modest horizontal spacing. The menu bar must span the full window width.

The toolbar starts immediately below this menu bar.

## 3. Main Toolbar — Critical Correction

The toolbar is approximately 64 pixels tall and spans the full application width. It contains nine large, evenly spaced command buttons arranged horizontally in this exact order:

1. Add URL
2. Start
3. Pause
4. Stop All
5. Resume All
6. Delete
7. Delete All
8. Options
9. Scheduler

Each toolbar button has:
- A large icon, approximately 27–30 px.
- A short text label directly underneath the icon.
- A transparent or toolbar-matching background.
- Consistent width, vertical alignment, and spacing.
- A compact clickable area with no large card or tile surrounding it.

Use recognizable line icons:
- Add URL: blue outlined plus inside a circle.
- Start: gray outlined play triangle.
- Pause: gray outlined square or pause symbol consistent with the existing icon set.
- Stop All: gray outlined square.
- Resume All: green play symbol inside a green outlined circle.
- Delete: gray trash can.
- Delete All: gray trash can or equivalent bulk-delete icon.
- Options: gray gear.
- Scheduler: gray clock.

Use the existing icon library if available. Do not replace icons with emoji or text characters.

The toolbar buttons should be approximately 75–80 pixels wide, with the first seven occupying the left portion and Options and Scheduler occupying the remaining positions. Use a predictable grid or flex layout rather than arbitrary positioning.

The toolbar should resemble a traditional download manager command ribbon, not a modern web application's large centered action panel.

**Important:** Add URL must not be displayed as a single oversized button centered above the download list. It must be the first button on the far left of the toolbar, with its icon above its label.

All nine buttons must fit at the reference window width without horizontal scrolling. At smaller window widths, preserve sensible spacing and prevent overlap.

## 4. Main Content Layout

Below the toolbar, divide the application into two vertical regions:

- Left sidebar: approximately 146 pixels wide at the reference window size.
- Main download list: fills all remaining horizontal space.

Use a horizontal layout with a fixed or appropriately constrained sidebar width and a flexible main content region.

A thin vertical divider separates the sidebar from the download list.

### Left Sidebar

The sidebar begins immediately below the toolbar and extends down to the status bar.

At the top, show a `Categories` header. Place a small `X` close/collapse control at the far right of that header.

Below it, display these category entries in this exact order:

- All Downloads
- In Progress
- Completed
- Documents
- Music
- Video
- Programs
- Compressed

Each row should be approximately 28 pixels tall and contain:
- A small icon on the left.
- The category name.
- A small count badge aligned to the right.

Use these icon concepts:
- All Downloads: blue outlined folder.
- In Progress: outlined play icon.
- Completed: outlined checkmark or completion icon.
- Documents: page/document icon.
- Music: musical note.
- Video: filmstrip.
- Programs: monitor.
- Compressed: archive box.

The selected `All Downloads` row should have a muted blue highlight, blue icon, light text, and a count of `0`. Other rows should use subdued gray icons and text with small, dark gray count badges.

The count badges should be compact, approximately 20 pixels wide when displaying a single digit. They must align consistently on the right.

After the category entries, insert a subtle horizontal divider with some vertical breathing room.

Next, display a `Queue` heading and these entries:

- Scheduled
- Queued
- Paused

Each entry uses a small appropriate icon, a label, and a right-aligned count badge. All counts are `0` in the empty state.

Insert another horizontal divider below the queue section.

At the bottom of the sidebar, display `Trash` with a trash icon and a right-aligned count badge showing `95` for the reference state. Use the actual application data if available; do not hardcode the count in production.

The sidebar should remain compact. Do not use large row heights, oversized icons, or excessive padding.

## 5. Download List Header

The download list starts immediately to the right of the sidebar.

At its top, display a single horizontal column-header row approximately 27 pixels tall. Use a slightly darker or subtly differentiated background and a thin bottom border.

Display these columns in this order:

1. File Name
2. Search icon/column
3. Size
4. Status
5. Time left
6. Transfer rate
7. Last Try
8. Description

The file name column receives the largest share of the available width. The remaining columns receive appropriate widths based on their content.

Use small, approximately 11–12 px text. Align column headings vertically and keep their spacing tight.

Show a small downward-pointing sort indicator beside `Last Try`, as in the reference.

Column boundaries may use subtle separators, but the header must not look like a heavily bordered spreadsheet.

The header must span the full width of the main content area. It must not be centered in the window or separated from the download list by a large blank region.

Make columns responsive to window resizing while preventing text collisions. Horizontal scrolling is acceptable only when the window becomes too narrow to display the columns.

## 6. Empty Download State

When there are no downloads, the main content area below the column headers should remain a uniform dark charcoal background.

Place a two-line empty-state message approximately in the center of the download-list area, both horizontally and vertically within the available space below the headers.

First line:

`No downloads`

Second line:

`Click Add URL on the toolbar to start downloading.`

Styling:
- First line: approximately 17–18 px, medium gray, normal or medium weight.
- Second line: approximately 13–14 px, darker muted gray.
- Small vertical gap between the lines.
- Center-align both lines.

Do not place the empty-state message in a card, panel, bordered container, or large button.

Center it relative to the download-list content region, not the entire application window. The sidebar must not affect the centering calculation.

The empty-state message should disappear when downloads are present, using the application's existing download-list logic.

## 7. Bottom Status Bar

Add a narrow status bar across the full window width, approximately 25 pixels tall, at the very bottom of the application client area.

Use a subtle top border and the same dark theme.

Display:

`Ready.`

Align it to the left with approximately 7 pixels of horizontal padding and small, muted gray text.

The status bar must remain anchored to the bottom when the window is resized.

## 8. Sizing, Spacing, and Alignment

Use these approximate measurements as initial targets at a window size of 909 × 614 pixels:

- Title bar: native Windows title bar.
- Menu bar: 27 px.
- Toolbar: 64 px.
- Sidebar: 146 px wide.
- Download column header: 27 px.
- Sidebar category and queue rows: approximately 28 px each.
- Status bar: 25 px.
- Toolbar icons: 27–30 px.
- Sidebar icons: 14–16 px.
- Main UI font: Segoe UI or the application's existing Windows-native font.

All measurements are guidelines. Prefer the application's native layout system and DPI-aware sizing over rigid pixel coordinates.

The main window should look balanced, compact, and information-dense. Remove the large unused space currently consumed by the oversized Add URL section. The toolbar should occupy only the band directly below the menu bar.

## 9. Implementation Requirements

Before making changes:

1. Inspect the existing main-window component, layout hierarchy, stylesheets, and icon library.
2. Identify why Add URL is occupying the center of the upper area.
3. Determine whether the toolbar commands, sidebar, column headers, empty state, and status bar are separate components or part of one layout.
4. Reuse existing components and functionality wherever possible.

Then implement the changes using the project's existing framework and styling conventions.

Requirements:
- Do not rebuild the application from scratch.
- Do not change download engine behavior or data models.
- Do not remove any existing toolbar actions.
- Do not invent new functionality as part of this visual correction.
- Do not hardcode production download counts.
- Do not use absolute positioning for the entire interface.
- Do not add a large empty spacer above the toolbar or download list.
- Ensure the title bar, menu, toolbar, sidebar, list, and status bar occupy the correct vertical regions.
- Preserve keyboard accessibility, tooltips, and existing command bindings.
- Maintain proper high-DPI scaling.
- Keep the interface responsive when resized.

## 10. Acceptance Criteria

The work is complete only when all of the following are true:

- The nine toolbar commands appear in one horizontal row beneath the menu bar.
- Add URL is on the far left, with its icon above its label.
- The toolbar no longer consumes a large central area.
- The sidebar is approximately 16% of the reference window's client width.
- The download list occupies all remaining horizontal space.
- The eight category entries and three queue entries appear in the specified order.
- The download column headers appear directly below the toolbar, not beneath a large blank region.
- The empty-state text is centered within the download-list body.
- The status bar remains at the bottom.
- The dark theme, blue selection accent, and green Resume All accent match the specified palette.
- All existing toolbar commands continue to work.
- The window can be resized without overlapping controls, clipping labels unnecessarily, or breaking the layout.

After implementation, launch the application and inspect the actual rendered window. Make any necessary adjustments based on the result rather than assuming the layout is correct from the source code alone.

**Priority:** Match the layout and proportions first, then refine colors, icon sizes, typography, and small spacing details.

_
The three most important fixes
If your agent needs a shorter explanation of the current problems, emphasize these:
1. Toolbar: Replace the large, centered Add URL area with a compact, 64-pixel-high horizontal toolbar containing all nine commands.
2. Vertical layout: Put the menu, toolbar, download column headers, and download list directly beneath one another, with no oversized blank space.
3. Proportions: Use a roughly 146-pixel sidebar, a flexible download list, and a 25-pixel bottom status bar.
The agent should make these changes in the existing UI layout rather than trying to fix the appearance by adjusting individual margins randomly.