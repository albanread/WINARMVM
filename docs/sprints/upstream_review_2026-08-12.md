# Upstream review — MACVM `09210e3`, and what it means for the Windows port

*Reviewed 2026-08-12 against `upstream/main`, from `wg3-subfloor-fix` at
WG7 complete. Nothing merged; this decides what SHOULD be.*

## What upstream built since we last looked

Two bodies of work and a batch of demos:

| upstream | what it is |
|---|---|
| `f1e0a6e` … `09210e3` (SM0–SM4) | **Shared screen memory.** `GameCommand` becomes control-plane only; all bulk state — pixels AND text — moves to buffers the VM writes directly. A text plane, worker VMs writing disjoint bands of one screen, a per-scanline palette (copper), and the palette itself as memory. |
| `6536294` | **SUnit design** — `TestCase` in the world, a Tests tab, a headless runner, so world regressions gate `cargo test` the way VM regressions do. |
| `a81fa35`, `ae44e14`, `dea610c`, `fdc4d80`, `f689bad`, `b3a1c1c`, `45d/45e` | Life, Minesweeper, FreeCell, copper, attractor, Julia, plasma, text pages — the demos that exercise the above. |
| `545cde4` | *Our own* S2 trap-site fix, ported upstream. Nothing to do. |

Core diff against us: `src/runtime/alien.rs` (+98), `src/runtime/primitives.rs`
(+263), `src/embed.rs` (+2433), `src/compiler/emit.rs` (+164).

## The finding that matters: we already built SM1

**SM1 is "the text plane — a HUD is stores now, not commands".** WG6d built
exactly that, independently, for a different reason:

> the guest owns MEANING and writes codepoint+colour cells into a buffer the
> renderer owns; the renderer owns PIXELS.

That is SM1's thesis word for word, arrived at from the opposite direction —
upstream got there from *bandwidth* (one `Text` command per revealed number,
~100/frame for a board that had not changed), we got there from *correctness*
(two authorities computing the same pixel, four shipped defects). The two
arguments landing on one design is the strongest evidence either of them is
right.

Concretely, `winui_render`'s `Cell { cp, fg, bg }` grid, written by the guest
through `WinArena u32At:put:` and drawn by DirectWrite glyph runs, IS a text
plane. Three views already use it — Editor, Monitor, Debugger.

**So the Windows port does not need SM1. It needs SM0.**

## What to adopt, and what not to

**Adopt — the pixel plane (SM0), and the palette (SM4).** This is the half we
have no equivalent of, and it supersedes what WG8 currently plans. The WG8 row
says *GDI-blit Canvas*, which is the design upstream is moving AWAY from: a
blit is the three-copies-per-frame path (`ByteArray` → `Vec` → mutex → memcpy →
upload) that SM0 exists to delete. Building it now would be building the thing
we would then replace — the rework this project has consistently refused.

The Windows shape is already sitting there. `winui_render` owns a D3D11 device
and a flip-model swapchain per pane; a pixel plane is a `D3D11_USAGE_DYNAMIC`
texture whose `Map`ped pointer the guest wraps in an `Alien` and stores into
directly, sampled by the same D2D context that draws the cell grid. No new
device, no new window, no new seam — the same `MacvmRender*` shape with a
buffer address instead of a cell buffer.

**Adopt — SUnit (S1–S4).** It applies unchanged: the world is the world. And
it lands somewhere specific here — the Windows shell has a Browser, an Editor
that live-compiles through `flows`, and now a Debugger that fronts a halt. A
Tests tab completes that loop: edit a class, run its tests, click a failure,
end in the Debugger. Every piece but the tab exists.

**Do not adopt — the host halves as written.** `embed.rs`'s +2433 lines and the
demos are Metal and `cocoaui` bound: `MTLStorageModeShared`, `IndexedPane`,
`text_overlay.rs`, `CocoaUI` view registration. The *primitives* and the
`alien.rs` work are platform-neutral and are the guest-facing contract worth
sharing; the rest is the Mac's renderer, and ours is DirectWrite.

**Do not adopt yet — the demos.** They are the reason to build SM0, not a
sprint of their own. Life and Julia want a pixel plane; FreeCell and Minesweeper
want a text plane we already have. Port them AFTER SM0, as WG8's gallery.

## Changes to the plan

- **WG8's Canvas row is rewritten** — shared screen memory rather than a GDI
  blit, with the note above as the reason.
- **WG9 added** — SUnit and the Tests tab.
- **WG10 added** — the demo gallery, which is what proves WG8 and WG9 rather
  than a sprint that stands alone.

One standing caution, recorded because this port has now paid for it four
times: SM0's buffer is the guest storing bytes a GPU reads, and the moment the
guest computes *where* a pixel goes, we are back to two authorities over one
coordinate. The cell grid avoided that by making the renderer the only side
that knows a pixel's address. A pixel plane cannot — the guest is writing
pixels by definition — so the discipline moves to the BUFFER's shape: stride,
width and height come from the renderer and are never assumed by the guest.
