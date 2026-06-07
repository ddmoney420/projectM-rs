//! Behavioral tests for `pm-eval`, asserting ns-eel-compatible semantics.

use pm_eval::{Context, Program};

/// Compile + run with a fresh context, returning the program's value.
fn run(src: &str) -> f64 {
    let mut ctx = Context::new();
    ctx.eval_str(src).expect("eval failed")
}

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
}

#[test]
fn arithmetic_and_precedence() {
    approx(run("1 + 2 * 3"), 7.0);
    approx(run("(1 + 2) * 3"), 9.0);
    approx(run("2 ^ 3 ^ 2"), 512.0); // right-associative: 2^(3^2)
    approx(run("-2 ^ 2"), -4.0); // pow binds tighter than unary minus
    approx(run("10 % 3"), 1.0);
    approx(run("7 - 3 - 2"), 2.0); // left-associative
}

#[test]
fn nseel_divide_by_zero_is_zero() {
    approx(run("5 / 0"), 0.0);
    approx(run("5 % 0"), 0.0);
    approx(run("fmod(5, 0)"), 0.0);
}

#[test]
fn epsilon_equality() {
    approx(run("0.000001 == 0"), 1.0); // within 1e-5 -> equal
    approx(run("0.001 == 0"), 0.0); // outside epsilon
    approx(run("1 != 2"), 1.0);
}

#[test]
fn logical_and_truthiness() {
    approx(run("1 && 1"), 1.0);
    approx(run("1 && 0"), 0.0);
    approx(run("0 || 0"), 0.0);
    approx(run("!0"), 1.0);
    approx(run("!5"), 0.0);
    // Sub-epsilon values are "false".
    approx(run("!0.000001"), 1.0);
}

#[test]
fn bitwise_ops() {
    approx(run("6 & 3"), 2.0);
    approx(run("6 | 1"), 7.0);
}

#[test]
fn variables_and_compound_assignment() {
    let mut ctx = Context::new();
    ctx.eval_str("x = 10").unwrap();
    ctx.eval_str("x += 5").unwrap();
    ctx.eval_str("x *= 2").unwrap();
    approx(ctx.get("x"), 30.0);
}

#[test]
fn case_insensitive_identifiers() {
    let mut ctx = Context::new();
    ctx.eval_str("MyVar = 42").unwrap();
    approx(ctx.get("myvar"), 42.0);
    approx(ctx.eval_str("myVAR + 1").unwrap(), 43.0);
}

#[test]
fn named_constants() {
    approx(run("$pi"), std::f64::consts::PI);
    approx(run("$e"), std::f64::consts::E);
    // Also accessible without the `$`.
    approx(run("pi * 2"), std::f64::consts::TAU);
}

#[test]
fn statement_blocks_return_last() {
    approx(run("1; 2; 3"), 3.0);
    approx(run("a = 5; b = a * 2; b + 1"), 11.0);
    // Trailing semicolon is allowed.
    approx(run("4;"), 4.0);
}

#[test]
fn if_special_form_is_lazy() {
    // The untaken branch must not run (would divide by a guarded zero anyway).
    approx(run("if(1, 100, 200)"), 100.0);
    approx(run("if(0, 100, 200)"), 200.0);
    // Lazy: writing to `hit` only in the taken branch.
    let mut ctx = Context::new();
    ctx.eval_str("if(0, hit = 1, hit = 2)").unwrap();
    approx(ctx.get("hit"), 2.0);
}

#[test]
fn loop_accumulates() {
    // sum 1..=5 via megabuf accumulator
    let v = run("i = 0; s = 0; loop(5, i += 1; s += i); s");
    approx(v, 15.0);
}

#[test]
fn while_runs_until_false() {
    let v = run("i = 0; while(exec2(i = i + 1, i < 4)); i");
    approx(v, 4.0);
}

#[test]
fn megabuf_addressing() {
    // base[offset] addresses megabuf at floor(base + offset).
    let mut ctx = Context::new();
    ctx.eval_str("0[3] = 99").unwrap();
    approx(ctx.eval_str("0[3]").unwrap(), 99.0);
    approx(ctx.eval_str("1[2]").unwrap(), 99.0); // same address 3
    approx(ctx.eval_str("megabuf(3)").unwrap(), 99.0);
    // Unset cells read as zero.
    approx(ctx.eval_str("0[7]").unwrap(), 0.0);
}

#[test]
fn megabuf_compound_assignment() {
    let mut ctx = Context::new();
    ctx.eval_str("5[0] = 10; 5[0] += 5").unwrap();
    approx(ctx.eval_str("5[0]").unwrap(), 15.0);
}

#[test]
fn builtin_math() {
    approx(run("sqrt(16)"), 4.0);
    approx(run("sqrt(-1)"), 0.0); // ns-eel guards negative sqrt
    approx(run("abs(-3.5)"), 3.5);
    approx(run("min(2, 7)"), 2.0);
    approx(run("max(2, 7)"), 7.0);
    approx(run("sign(-9)"), -1.0);
    approx(run("int(3.9)"), 3.0);
    approx(run("int(-3.9)"), -3.0);
    approx(run("floor(3.9)"), 3.0);
    approx(run("pow(2, 10)"), 1024.0);
}

#[test]
fn rand_is_deterministic_with_seed() {
    let mut a = Context::new();
    a.seed(12345);
    let mut b = Context::new();
    b.seed(12345);
    let prog = Program::compile("rand(1000)").unwrap();
    for _ in 0..50 {
        let va = prog.run(&mut a).unwrap();
        let vb = prog.run(&mut b).unwrap();
        assert_eq!(va, vb);
        assert!((0.0..1000.0).contains(&va));
        assert_eq!(va, va.floor()); // rand(x) returns an integer in [0, x)
    }
}

#[test]
fn comments_are_ignored() {
    approx(run("1 + /* inline */ 2 // trailing\n + 3"), 6.0);
}

#[test]
fn realistic_per_frame_snippet() {
    // A fragment in the style of an actual Milkdrop per_frame block.
    let src = "
        wave_r = 0.5 + 0.5 * sin(time * 1.3);
        wave_g = 0.5 + 0.5 * sin(time * 1.7 + 2);
        wave_b = 0.5 + 0.5 * sin(time * 1.9 + 4);
        zoom = 1.0 + 0.02 * sin(time);
    ";
    let prog = Program::compile(src).unwrap();
    let mut ctx = Context::new();
    ctx.set("time", 1.0);
    prog.run(&mut ctx).unwrap();
    // All channels stay in [0, 1].
    for ch in ["wave_r", "wave_g", "wave_b"] {
        let v = ctx.get(ch);
        assert!((0.0..=1.0).contains(&v), "{ch} = {v} out of range");
    }
    approx(ctx.get("zoom"), 1.0 + 0.02 * 1.0_f64.sin());
}
