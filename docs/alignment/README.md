# Alignment workspace (U-Type fork)

This branch adds **Photo → Align Photos…** for independently exporting aligned before/after photographs. It does not stitch or blend them into a panorama and never changes an original or its Develop settings.

## Workflow — custom UI v0.1

1. Import and select 2–32 photos. Make the intended reference active, then press the blue **Align Photos v0.1** button immediately left of Appearance Mode. The Photo menu opens the same workspace. Selection loads automatically; an existing modified set prompts before replacement.
2. The resizable left panel contains Project, Set, Alignment and Export sections. Drag its right edge to resize. The Set list highlights the photo in the active viewport. Click a viewport to focus it (blue outline), then choose a photo in the list. Choosing a different reference remaps existing landmarks into that reference and resets the common crop; it requires a fitted replacement if landmarks already exist. Review and undo this operation if necessary, especially for local warps.
3. **Add photos…** opens a searchable thumbnail grid of imported catalog photos. Select multiple tiles and press Add selected. **Import new photos…** opens the normal import workflow; return to the picker to add the newly imported photos. Remove selected photo removes only set membership, not a catalog photo or its file. Keep at least two photos. Removing a reference chooses a fitted replacement (or the next photo when no landmarks exist).
4. Place landmarks in the upper left reference and upper right target: click left, then the corresponding position right. Drag markers to refine. Rotation + scale needs two distinct pairs; affine three non-collinear pairs; perspective four well-distributed pairs. Local warp changes shape; retain unwarped comparisons when interpreting anatomical changes. Suggest points works only on an empty target.
5. The horizontal **display options bar** stays above the viewports. Wheel over a viewport to zoom around the cursor; right-drag to pan. Toggle **Linked zoom / pan** for the upper pair. The lower result has independent navigation; **Lock result zoom / pan** couples it to the upper views. **Invert wheel zoom** reverses direction. Fit active resets the focused viewport and any linked views.
6. The lower viewport continuously shows Overlay, Split, Difference or Blink while the upper landmark views remain visible. Click the result viewport then a Set list entry to choose its target independently. The reference can also be shown alone here. The yellow rectangle marks the shared output crop.
7. In Export, use Common crop or edit reference-relative x/y/width/height. Set Width, then Match crop aspect. Pick an output folder and Export separate images. Current Develop edits are rendered from the originals; all outputs share framing and dimensions. PNG preserves uncovered pixels as transparent; JPEG refuses them. Existing exports are never overwritten.

## Projects, working backups and closing

The proposed project path is beside the reference photo (also for mixed-folder sets). Filename stems are joined with `_` when the combined name is at most 15 Unicode characters; otherwise the first stem is truncated to 15 characters followed by `+N`, the number of additional photos. The extension is `.lcalign.json`. Unrelated sets with the same suggested name get a numbered suffix rather than being overwritten.

Every successful alignment edit, membership change and undo/redo writes a separate `NAME.working.lcalign.json` backup atomically. A backup never marks the project explicitly saved and never replaces an existing saved checkpoint. Write failures remain visible, with edits retained in memory. Demo photos have no source folder: use Save As to enable backups.

Closing the alignment window, quitting LightCraft, or replacing a modified set asks **Save / Discard / Save As / Cancel**. Save writes the checkpoint; Save As writes to the chosen filename and then completes the pending close; cancelling the file picker leaves the close prompt open. Cancel preserves the workspace. Discard removes the working copy and leaves the last saved checkpoint unchanged. Errors keep the prompt open.

Opening the same selection after a crash restores its matching working copy. Open can also select a working copy explicitly; its saved-project path is normalized back to the ordinary `.lcalign.json` name. All sidecars require the same catalog/source identities. Reference changes or changed membership may change the suggested filename on a later new selection; use Open for those saved sets.

Close the alignment window to work in Develop, then reopen it. Photometric changes refresh previews while retaining landmarks. Geometry changes block export/comparison until **Remap landmarks through changed geometry** or **Reset affected landmarks**. Remapping a point outside the new crop reports an error; a changed reference resets the shared crop. These operations are undoable.

## Current boundaries

- Desktop implementation; the web UI reports unsupported rather than attempting worker threads.
- Export is SDR sRGB 8-bit PNG or JPEG (quality 95), without original metadata. This avoids propagating location/patient-identifying EXIF by default. The sidecar itself contains original filenames/paths and must be handled with the photos.
- Preview long edge is 1200 px; exports render from the original with current Develop settings at native developed resolution. The preview is never used as the export source. The final alignment sampler is bilinear; high-bit-depth output and more advanced minification filtering remain follow-up quality work.
- Local warp uses a regularized thin-plate spline. Sampled Jacobian checks reject folds/collapse; this is not a proof that every unsampled location is fold-free. Common crop is a conservative 256×256 grid approximation, particularly for local warps.
- Maximum 128 pairs per target, 32 members, and 100 million output pixels. Full developed sources above 100 MP are refused. Full-resolution export processes one image at a time. Cancel stops between images; completed files remain and the error reports their count.
- Changing the reference remaps landmark coordinates using the existing fitted transform, then refits each member. Review results after rebasing, especially local warps. Source identity changes or a different catalog require creating/rebinding the set rather than silently guessing matches.
- Alignment undo/redo is separate from ordinary Develop undo/redo. Explicit saves and automatic working backups are separate; modified sets prompt on close or quit.
- The initial alignment implementation was successfully built and tested by the user on Windows. This v0.1 UI overhaul still needs Windows acceptance testing. Synthetic tests do not establish clinical accuracy.

## Layout and maintenance

| Module | Responsibility |
|---|---|
| `crates/merge/src/registration/` | Aspect-correct weighted transforms, local warp, sampling and shared crop |
| `crates/engine/src/alignment/` | Versioned sidecars, geometry mapping, detached preview/export jobs |
| `crates/engine/src/cmd/alignment.rs` | Shared command API (`alignment.*`) |
| `crates/ui-egui/src/alignment.rs` | Point editing and comparison workspace |

There are no new third-party dependencies or catalog migrations. The fork adds small registration hooks in the existing session, command registry, app initialization, frame and Photo menu. Quit/library-switch hooks protect unsaved sets. Official update checks are disabled by `alignment::CUSTOM_BUILD`, because official binaries would remove this feature.

Keep `main` tracking upstream and use `alignment` for the custom app. For each upstream update, create a temporary integration branch from `alignment`, merge `upstream/main`, resolve conflicts, run `cargo xtask ci` and the manual acceptance steps below, then merge the tested result back. Never replace fork dependency pins merely to resolve a build error. No changes to GitHub Actions are needed.

## Acceptance checks

- Two images: align with two well-spaced pairs; drag a point; undo/redo; save/reopen.
- Three or more images: solve each target independently; disable/include members; common crop; matching-size individual exports.
- Change exposure in Develop, refresh, and verify landmarks remain. Rotate/crop, remap, and verify anatomical locations; undo remapping.
- Coincident/collinear points, NaN/corrupt sidecars, an unrelated library, and missing originals must report errors.
- Verify PNG transparency and JPEG uncovered-area rejection. Export twice to the same folder: the second attempt must refuse to overwrite.
- Close/quit with an unnamed modified set and with a running export; cancellation must preserve the set.
- Review local-warp results against the original photographs and representative viewpoints/lighting.

## Build numbering

`crates/ui-egui/src/alignment.rs` owns `BUILD_VERSION`. This first numbered UI build is **v0.1**. Increment once per subsequent prepared user build (v0.2, v0.3, ...), not per development commit. Preserve this convention when preparing updates; the main application version remains upstream's version.
