; C+ Phase 0 — hand-written hello world
; Frozen IR proving the backend pipeline: cpc -> clang -> binary.
; No parser, no codegen yet; this file is emitted verbatim.

@.str = private unnamed_addr constant [14 x i8] c"hello, world\0A\00", align 1

declare i32 @printf(ptr noundef, ...)

define i32 @main() {
entry:
  %0 = call i32 (ptr, ...) @printf(ptr noundef @.str)
  ret i32 0
}
