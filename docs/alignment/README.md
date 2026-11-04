# Alignment workspace (U-Type fork)

This branch adds **Photo → Align Photos…** for independently exporting aligned before/after photographs. It does not stitch or blend them into a panorama and never changes an original or its Develop settings.

## Workflow

1. Import photographs into a library, select 2–32, and make the intended reference the active selection.
2. Open **Photo → Align Photos…**, then **New from selection**. The first member is the reference.
3. Choose a target. Click a landmark in the left image, then its counterpart on the right. Repeat. Drag numbered markers or edit their normalized coordinates in the table. Right-drag pans both images; Linked zoom and the cursor magnifier help placement.
4. Start with **Rotation + scale** (two distinct pairs). One pair supports position only; affine needs three non-collinear pairs; perspective needs four well-distributed pairs. Additional pairs produce a weighted least-squares fit. Individual pairs can be disabled or weighted; manual pairs are never silently discarded.
5. With an empty target, **Suggest points** tries feature matching plus deterministic RANSAC. Failure leaves the project unchanged. Review the proposed landmarks and fit visually. Automatic matching is an aid, not a guarantee for different viewpoints or lighting.
6. Review using Overlay, Split, Difference or Blink. Point pairs always shows the unwarped developed photographs. Local warp is explicitly selected and stored in the project; it can change the apparent shape of anatomy. Use unwarped comparisons when interpreting before/after changes.
7. Use **Common crop**, or enter reference-relative x/y/width/height. The yellow rectangle in comparison views marks the output crop. Set output width and use **Match crop aspect** to avoid stretching; independent dimensions are available intentionally.
8. Choose a project filename ending in `.lcalign.json` and **Save**. Save before opening another set. A saved set can be reopened after restarting with the same library. Multiple sidecars can reference the same photos independently.
9. Choose a new output folder and **Export separate images**. Exports have identical dimensions and framing, ordered names, and the photos' current Develop edits. Existing files and originals are never overwritten. PNG preserves uncovered areas as transparent; JPEG refuses uncovered areas, so use Common crop first.

Close the Align window to work in normal Develop, then reopen it. Photometric edits invalidate only the displayed preview; click Refresh edits / previews. Geometry changes block comparison/export until reviewed: **Remap landmarks through changed geometry** maps points through their source coordinates. If a point falls outside the new crop, widen the crop or use **Reset affected landmarks**. A change to the reference geometry resets the shared crop; review it again. Remapping and resets have alignment undo/redo.

## Current boundaries

- Desktop implementation; the web UI reports unsupported rather than attempting worker threads.
- Export is SDR sRGB 8-bit PNG or JPEG (quality 95), without original metadata. This avoids propagating location/patient-identifying EXIF by default. The sidecar itself contains original filenames/paths and must be handled with the photos.
- Preview long edge is 1200 px; exports render from the original with current Develop settings at native developed resolution. The preview is never used as the export source. The final alignment sampler is bilinear; high-bit-depth output and more advanced minification filtering remain follow-up quality work.
- Local warp uses a regularized thin-plate spline. Sampled Jacobian checks reject folds/collapse; this is not a proof that every unsampled location is fold-free. Common crop is a conservative 256×256 grid approximation, particularly for local warps.
- Maximum 128 pairs per target, 32 members, and 100 million output pixels. Full developed sources above 100 MP are refused. Full-resolution export processes one image at a time. Cancel stops between images; completed files remain and the error reports their count.
- A reference is chosen at creation. To change it, create another set. Source identity changes or a different catalog require creating/rebinding the set rather than silently guessing matches.
- Alignment undo/redo is separate from ordinary Develop undo/redo. Save is explicit; quitting retries saving a previously named set, or prompts before discarding unsaved work. Closing the workspace window keeps its in-memory set.
- Actual Windows runtime and representative dental-photo acceptance testing are still required. Synthetic tests do not establish clinical accuracy.

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
