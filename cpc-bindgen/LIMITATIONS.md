# cpc-bindgen — remaining gaps

The ObjC front-end (`--objc`) covers the common Objective-C binding constructs:
classes, init/factory (incl. nullable factory to `Option[Self]`), ARC ownership,
`str`/`Text`, nullable to `Option`, `NS_ENUM`, `NSRange`, `NSArray` to/from `Vec`
(both directions), `BOOL`, `f64` (double/CGFloat/NSTimeInterval), any-arity
selectors, categories, blocks (`usingBlock:`), and delegate/data-source
protocols (void and non-void callbacks, multi-method, override-named).

Two gaps remain. Each SKIP site in the generator points back to this file.

## 1. NSDictionary returns — blocked on stdlib

A method returning `NSDictionary<NSString*, V>*` (e.g.
`-[NSLanguageRecognizer languageHypothesesWithMaximum:]`,
`+[NSTimeZone abbreviationDictionary]`) is emitted as a `// SKIPPED` comment.

**Why it is blocked.** The natural target type is a map keyed by `Text`. The
stdlib `HashMap[K, V]` requires `K: Copy` (`vendor/stdlib/src/hash_map.cplus`,
`struct HashMap[K: Copy, V: Copy]`), and `Text` is not `Copy` (it owns a heap
buffer and has `drop`). So `HashMap[Text, V]` does not type-check. There is no
other Text-keyed associative type in stdlib today.

**What unblocks it (stdlib work).** Any one of:
- A Text-keyed map whose key bound is `Hash + Eq` rather than `Copy` (keys moved
  in / borrowed for lookup, not bitwise-copied), or
- Relaxing `HashMap` to take non-`Copy` keys with `Clone` semantics, or
- A purpose-built `StringMap[V]` (Text keys, hashing over the bytes).

`Text` already has the pieces a hasher needs (`count`, `byte_at`, `c_str`).

**Where to wire it once stdlib is ready (cpc-bindgen):**
1. `src/objc.rs`, `map_ret`: the `NSDictionary<...>` / `NSMutableDictionary<...>`
   branch currently returns `Ret::Unsupported(...)`. Add a `Ret::TextMap`
   variant (mirror `Ret::TextArray`) and detect string-keyed dictionaries the
   way `is_string_array` detects string arrays.
2. Emit a loop like the `TextArray` return path (around the `Ret::TextArray`
   arm): iterate `allKeys` (an `NSArray`), `objectForKey:` per key, bridge each
   key/value, insert into the new map.
3. Add a `bridge::` helper for the value side (e.g. NSNumber to scalar) if the
   value type is not already an object.
4. The reverse (Dictionary *param*) mirrors `bridge::nsarray_of_text` in
   `vendor/objc/src/bridge.cplus`: build an `NSMutableDictionary` and
   `setObject:forKey:` per entry.

The `Vec[Text]` bridge (return: the `Ret::TextArray` path in `src/objc.rs`;
param: `bridge::nsarray_of_text`) is the working template to copy.

## 2. 32-bit `int` / `float` scalars — deferred (low priority)

Raw `int` / `unsigned int` / `float` params and returns are emitted as
`// SKIPPED`. Modern Cocoa uses `NSInteger` (i64), `NSUInteger` (u64), `CGFloat`
(f64), `BOOL`, and `id`, so this is rare and low value, but mechanical to add:

1. `vendor/objc/src/runtime.cplus`: add msgSend shims for the i32/u32/f32 widths
   needed (e.g. `objc_msg_i32`, `objc_msg_id_i32`, ...), with wrappers, the same
   way the i64/u64/f64 shims are defined.
2. `src/objc.rs`: add `Ret::ScalarI32` / `Ret::ScalarF32` (and `Arg` variants),
   map `int`/`unsigned`/`float` to them in `map_ret`/`map_arg`/`param_sig_type`,
   and add the wire tags to the `KNOWN` list in `send_expr`.

These live next to the existing `ScalarI64`/`ScalarU64`/`ScalarF64` handling, so
the diff follows an existing pattern at each site.

## Not gaps

Blocks and delegate/data-source synthesis (the two hard callback mechanisms) are
fully implemented and live-tested (`vendor/delegate_proof`, `vendor/nl_gen`).
Non-void delegate return shapes are curated in `vendor/objc/src/synthesis.cplus`
(`add_method_<ret>_<n>id`) and gated by `delegate_shape_known` in `src/objc.rs`;
to support a new (return type, arg count) callback shape, add the
`class_addMethod` extern + wrapper in synthesis.cplus and the pair to
`delegate_shape_known`.
