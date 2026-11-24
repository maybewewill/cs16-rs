(module
  (import "cs16_mod" "hook_register" (func $hook_register (param i32 i32 i32) (result i32)))
  (import "cs16_mod" "send_custom_usermsg" (func $send_custom (param i32 i32 i32 i32 i32) (result i32)))
  (import "cs16_mod" "timer_repeat" (func $timer_repeat (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "CsModHudTick")
  (data (i32.const 16) "\01\55")

  (func (export "cs_mod_init") (result i32)
    (drop (call $hook_register (i32.const 1) (i32.const 1) (i32.const 0)))
    (i32.const 0)
  )

  (func (export "cs_mod_hook") (param $hid i32) (param $phase i32) (result i32)
    (if (i32.eq (local.get $hid) (i32.const 1))
      (then
        (drop (call $timer_repeat (i32.const 50) (i32.const 1) (i32.const 0)))
      )
    )
    (i32.const 0)
  )

  (func (export "cs_mod_timer") (param $cb i32) (param $arg i32) (result i32)
    (drop (call $send_custom (i32.const -1) (i32.const 0) (i32.const 12) (i32.const 16) (i32.const 2)))
    (i32.const 0)
  )
)
