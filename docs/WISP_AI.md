# Running trained models in WISP

This document is the pattern behind `esp/APPS/xor_brain.wisp` and
`esp/APPS/tamer.wisp`: how to take a small model, trained offline on a
real computer, and run its forward pass (inference only -- WISP has no
training/backprop machinery, and never will; that's the wrong thing to
build in a bare-metal tree-walking interpreter) entirely inside the UEFI
shell, on real firmware, with no OS and no floating point.

If you're an LLM being handed this document to write a new WISP model:
read this file plus `docs/WISP_LANGUAGE.md` in full, then look at
`tamer.wisp` as a worked example before writing anything.

## What this is, and isn't

This runs real trained neural networks -- real weights, real matrix
multiply, a real nonlinearity, a real forward pass, checked against the
model's actual training accuracy before anything ships. It is not, and
will not become, anything resembling a language model. There's no
tokenizer, no attention, no way to load a `.safetensors`/`.gguf` file
(WISP scripts can't read arbitrary files at all -- see
`docs/WISP_LANGUAGE.md`'s "What's not in v1"), and no realistic path to
one: real LLM inference leans on years of SIMD-tuned matmul kernels and
usually GPU acceleration, none of which exist pre-OS. What's here is the
other end of the size spectrum on purpose -- tiny, fully-owned, fully
verified models, sized to what a tree-walking interpreter on bare metal
can actually do.

## The pattern, end to end

1. **Train (or derive) the model on a real computer, in Python, with
   whatever tools you like** -- this repo's two examples use plain numpy
   with hand-rolled backprop (`tools/train_tiny_nn.py`), but nothing
   about the pattern requires that specifically; the only constraint is
   that you end up with a small, fixed set of numeric weights.
2. **Verify the model's accuracy in float**, the normal way, before
   touching fixed-point at all.
3. **Quantize every weight and bias to a fixed-point integer**: pick a
   `SCALE` (both examples use `1000`, giving 3 decimal digits of
   precision -- plenty for models this size; go higher if you have
   weights that need finer resolution and are confident you won't
   overflow `i64` once things get multiplied together, which for models
   this tiny is not a realistic risk), then store `round(weight * SCALE)`
   as a plain WISP `int`.
4. **Re-verify accuracy using the exact integer arithmetic WISP will
   run** -- not the float math from step 2. This is the step that
   actually matters, and it caught a real bug during this project:
   Python's `//` is floor division, but WISP's `/` compiles to Rust's
   `i64::checked_div`, which truncates toward zero. Those disagree on
   negative operands. A verification script that reference-implements
   `fixed_mul` with Python's `//` will report accuracy for a rounding
   behavior WISP doesn't actually have. `tools/train_tiny_nn.py`'s
   `_trunc_div` helper exists specifically to avoid this trap -- copy it
   (or the pattern) rather than re-deriving fixed-point rounding from
   scratch.
5. **Only after both verifications pass**, write the WISP program: bake
   the quantized weights in as list literals, write `fixed_mul` and the
   forward pass, and drive it from `input()` the same as any other
   interactive `.wisp` program.
6. **Test the shipped WISP code itself**, not just the Python that
   produced its constants -- add cases to `wisp_harness` that exercise
   specific, individually-checkable predictions (not just "it runs"),
   ideally including a boundary case where being off by one quantization
   step would flip the answer (`tamer.wisp`'s harness tests include
   `(5, 3)` vs `(6, 3)`, exactly either side of its trained threshold).

## Fixed-point arithmetic in WISP

WISP has exactly one numeric type, `int` (64-bit signed), and no floats.
Fixed-point is a *convention* on top of that, not a language feature --
nothing below needs interpreter support, it's plain WISP:

- A real number `r` is represented as the integer `round(r * SCALE)`.
- **Adding/subtracting** two fixed-point values at the same `SCALE` is
  just `+`/`-` -- the scale factor cancels out automatically.
- **Multiplying** two fixed-point values needs a divide-back-down step,
  because multiplying two numbers each carrying a factor of `SCALE`
  leaves the product carrying `SCALE^2`:
  ```
  fn fixed_mul(a, b) {
      let prod = a * b;
      if prod >= 0 {
          return (prod + SCALE / 2) / SCALE;
      }
      return (prod - SCALE / 2) / SCALE;
  }
  ```
  The `+ SCALE / 2` / `- SCALE / 2` before dividing rounds to the
  nearest representable fixed-point value instead of always truncating
  toward zero, which matters once you compose more than one
  multiplication (`xor_brain.wisp` doesn't need this at all -- its
  weights are exact integers; `tamer.wisp`'s output layer does, since
  it's the one place a fixed-point weight is multiplied against a
  fixed-point *activation* rather than a plain exact-integer input --
  see the comment at the top of that file for exactly which
  multiplications need it and which don't).
- **Multiplying a fixed-point value by a plain exact integer** (a loop
  counter, a small input like `tamer.wisp`'s 0-10 stats) needs no
  `fixed_mul` at all -- the scale factor just carries straight through,
  since only one side of the multiplication has it.
- **ReLU** (`max(0, x)`) is scale-invariant -- it's just a comparison and
  a select, so it works identically on a fixed-point value as on a plain
  int, with no special-casing needed. This is a big part of why these
  two demos use ReLU rather than a smooth activation like sigmoid/tanh:
  those need a genuine nonlinear function approximation (a lookup table
  or a fixed-point polynomial fit) to implement without floats, which is
  real extra work and its own source of quantization error, whereas
  ReLU is exact in any fixed-point scale for free.
- **Thresholding a fixed-point value against a real cutoff** (e.g. "is
  the output > 0.5?") just means comparing against `SCALE / 2` (or
  whatever the cutoff scales to) rather than the literal target -- see
  `tamer.wisp`'s `y > (SCALE / 2)`.

## Representing a model's weights

Use nested lists (`docs/WISP_LANGUAGE.md`'s "Lists" section) rather than
a pile of individually-named variables per unit -- `tamer.wisp`'s
`W1 = [[145, 198], [229, 189], [-47, 325], [47, 317]]` is a real 4x2
weight matrix, and its forward pass walks it with a `while` loop over
`len(B1)` rather than manually unrolling four copies of the same
arithmetic. This is the actual reason lists were worth adding to the
language at all: without them, every model would need its architecture
hardcoded into the shape of the WISP source itself (a fixed number of
copy-pasted `hN`/`wN` variables), which stops scaling almost immediately
and makes the code say nothing about what it's actually computing.

## What's next, if you want to keep pushing this

- **Multiple hidden layers.** Nothing here is limited to one hidden
  layer -- chain another `W2`/`B2`-shaped pair and another `relu`/loop
  pass. The only real cost is more `fixed_mul` calls and, eventually,
  wanting nested lists of hidden-layer outputs rather than a flat one.
- **A lookup-table activation function**, if a model genuinely needs
  sigmoid/tanh rather than ReLU (e.g. because you want output values
  smoothly bounded in `(0, 1)` rather than an unbounded linear output
  thresholded after the fact). A small fixed-point lookup table (say 32
  or 64 entries covering a reasonable input range, with linear
  interpolation between entries) is the standard bare-metal approach and
  is itself just a WISP list plus some index arithmetic -- no
  interpreter changes needed, same as everything in this document.
- **Larger/deeper networks** are ultimately limited by two things: how
  much of the model's source-of-truth (its weights) you're willing to
  hand-transcribe into a `.wisp` list literal (WISP scripts can't read
  arbitrary data files, so there's no "load weights.bin" shortcut yet --
  see `docs/WISP_LANGUAGE.md`'s file I/O limitation), and how many
  `fixed_mul` calls the tree-walking interpreter can get through in a
  time budget a human waiting at a `>` prompt will tolerate. Both are
  soft limits, not hard walls -- they're exactly the kind of thing this
  whole project has been about hand-building solutions to, one piece at
  a time.
