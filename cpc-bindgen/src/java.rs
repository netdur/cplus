//! Java frontend: bind JVM classes (Android's `android.jar`, or any classpath)
//! into C+ wrappers over `vendor/jni`.
//!
//! The metadata source is `javap -s`, shelled out to exactly the way `objc.rs`
//! shells out to clang and `swift.rs` to `swift symbolgraph-extract`. Java's
//! metadata is *more* machine-readable than ObjC's, not less: a class file
//! stores the JVM descriptor verbatim, so `(Ljava/lang/String;)V` — precisely
//! what `GetMethodID` wants — is read, never synthesized.
//!
//! Following the house rule: anything we cannot model becomes a
//! `// SKIPPED <name>: <reason>` line, never wrong code.
//!
//! Emitted shape, per class:
//!
//! ```text
//! struct TextView { _env: rt::Env, _obj: jni::jobject }   // owns a GLOBAL ref
//! impl TextView { drop / from_local / from_global / as_obj / into_raw
//!                 + one fn per constructor, method and static final field }
//! ```
//!
//! Every call routes through `rt::Env::method`, which caches the class and
//! method id. That is deliberate and measured: resolving per call costs a clean
//! 2x on a 400-node mount (plans/plan.android.md rung 1).

use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub desc: String,
    pub is_static: bool,
    pub is_ctor: bool,
    pub is_field: bool,
}

#[derive(Debug, Clone)]
pub struct Class {
    pub fqn: String,
    pub simple: String,
    /// `extends` target, if any. Used for the typed upcast — `javap` lists only
    /// DECLARED members, so a `TextView` binding has no `setVisibility` of its
    /// own and reaches `View`'s through `as_view()`. Same shape the ObjC
    /// generator uses.
    pub super_fqn: Option<String>,
    pub members: Vec<Member>,
}

/// Is this a name we can emit a function for? javap prints a class's static
/// initializer as `static {};`, whose "name" parses out as `{}` — and a few
/// synthetic members are similar. Anything that is not a plain Java identifier
/// becomes a `// SKIPPED` line rather than syntactically invalid C+.
pub fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    cs.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// `android.widget.TextView` -> `android/widget/TextView` (the JNI form).
pub fn jni_name(fqn: &str) -> String {
    fqn.replace('.', "/")
}

/// `setTextSize` -> `set_text_size`; `getX` -> `get_x`; `setARGB` -> `set_argb`.
pub fn snake(name: &str) -> String {
    let ch: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for i in 0..ch.len() {
        let c = ch[i];
        if c.is_ascii_uppercase() {
            let prev_lower = i > 0 && (ch[i - 1].is_ascii_lowercase() || ch[i - 1].is_ascii_digit());
            let next_lower = i + 1 < ch.len() && ch[i + 1].is_ascii_lowercase();
            let prev_upper = i > 0 && ch[i - 1].is_ascii_uppercase();
            if !out.is_empty() && (prev_lower || (prev_upper && next_lower)) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if c == '$' {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    out
}

/// Split a JVM method descriptor into its parameter types and return type.
/// `(Ljava/lang/String;I)V` -> (["Ljava/lang/String;", "I"], "V")
pub fn split_descriptor(desc: &str) -> Option<(Vec<String>, String)> {
    let bytes: Vec<char> = desc.chars().collect();
    if bytes.first() != Some(&'(') {
        return None;
    }
    let mut i = 1usize;
    let mut params = Vec::new();
    while i < bytes.len() && bytes[i] != ')' {
        let start = i;
        while i < bytes.len() && bytes[i] == '[' {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == 'L' {
            while i < bytes.len() && bytes[i] != ';' {
                i += 1;
            }
            if i >= bytes.len() {
                return None;
            }
            i += 1;
        } else {
            i += 1;
        }
        params.push(bytes[start..i].iter().collect::<String>());
    }
    if i >= bytes.len() {
        return None;
    }
    let ret: String = bytes[i + 1..].iter().collect();
    Some((params, ret))
}

/// How one JVM type crosses the boundary.
pub struct Mapped {
    /// The C+ parameter/return type.
    pub cplus: String,
    /// `jni::arg_*` constructor for a parameter, or "" if it needs a jstring.
    pub arg_ctor: &'static str,
    /// Which `Call<T>MethodA` slot a return of this type uses.
    pub call_kind: &'static str,
    /// A short, stable tag used to disambiguate overloads.
    pub tag: String,
    /// A `java.lang.String`/`CharSequence` parameter: taken as NUL-terminated
    /// modified UTF-8 and turned into a jstring at the call.
    pub is_string: bool,
}

pub fn map_type(t: &str) -> Option<Mapped> {
    let simple = |cplus: &str, ctor: &'static str, kind: &'static str, tag: &str| {
        Some(Mapped {
            cplus: cplus.to_string(),
            arg_ctor: ctor,
            call_kind: kind,
            tag: tag.to_string(),
            is_string: false,
        })
    };
    match t {
        "V" => simple("", "", "Void", "v"),
        "Z" => simple("bool", "jni::arg_bool", "Boolean", "z"),
        "B" => simple("i8", "jni::arg_int", "Byte", "b"),
        "C" => simple("u16", "jni::arg_int", "Char", "c"),
        "S" => simple("i16", "jni::arg_int", "Short", "s"),
        "I" => simple("i32", "jni::arg_int", "Int", "i"),
        "J" => simple("i64", "jni::arg_long", "Long", "l"),
        "F" => simple("f32", "jni::arg_float", "Float", "f"),
        "D" => simple("f64", "jni::arg_double", "Double", "d"),
        "Ljava/lang/String;" | "Ljava/lang/CharSequence;" => Some(Mapped {
            cplus: "*u8".to_string(),
            arg_ctor: "",
            call_kind: "Object",
            tag: "str".to_string(),
            is_string: true,
        }),
        _ => {
            if let Some(inner) = t.strip_prefix('[') {
                let tag = map_type(inner).map(|m| m.tag).unwrap_or_else(|| "obj".into());
                return Some(Mapped {
                    cplus: "jni::jarray".to_string(),
                    arg_ctor: "jni::arg_object",
                    call_kind: "Object",
                    tag: format!("arr{tag}"),
                    is_string: false,
                });
            }
            if t.starts_with('L') && t.ends_with(';') {
                let fqn = &t[1..t.len() - 1];
                let last = fqn.rsplit('/').next().unwrap_or("obj");
                return Some(Mapped {
                    cplus: "jni::jobject".to_string(),
                    arg_ctor: "jni::arg_object",
                    call_kind: "Object",
                    tag: snake(last),
                    is_string: false,
                });
            }
            None
        }
    }
}

// ---- javap ------------------------------------------------------------------

fn javap_bin() -> String {
    if let Ok(h) = std::env::var("JAVA_HOME") {
        let p = format!("{h}/bin/javap");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    "javap".to_string()
}

/// Run `javap -s` over `classes` and parse the result.
pub fn read_classes(classpath: &str, classes: &[String]) -> Result<Vec<Class>, String> {
    let mut cmd = Command::new(javap_bin());
    cmd.arg("-s");
    if !classpath.is_empty() {
        cmd.arg("-classpath").arg(classpath);
    }
    for c in classes {
        cmd.arg(c);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run javap ({e}). Set JAVA_HOME or put javap on PATH."))?;
    if !out.status.success() {
        return Err(format!(
            "javap failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_javap(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `javap -s` output. Each member is a declaration line ending in `;`
/// followed by an indented `descriptor:` line — the JVM descriptor, which is
/// exactly what `GetMethodID` / `GetFieldID` take.
pub fn parse_javap(text: &str) -> Vec<Class> {
    let mut classes: Vec<Class> = Vec::new();
    let mut pending: Option<(String, bool)> = None; // (decl line, is_static)
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("Compiled from") {
            continue;
        }
        if let Some(desc) = t.strip_prefix("descriptor: ") {
            let (decl, is_static) = match pending.take() {
                Some(p) => p,
                None => continue,
            };
            let cls = match classes.last_mut() {
                Some(c) => c,
                None => continue,
            };
            let is_field = !desc.starts_with('(');
            // The declared name is the token before `(` for a method, or the
            // last token before `;` for a field.
            let head = decl.trim_end_matches(';');
            let name = if is_field {
                head.rsplit(|c: char| c.is_whitespace())
                    .next()
                    .unwrap_or("")
                    .to_string()
            } else {
                let before = head.split('(').next().unwrap_or("");
                before
                    .rsplit(|c: char| c.is_whitespace())
                    .next()
                    .unwrap_or("")
                    .to_string()
            };
            // A constructor's "name" is the fully-qualified class name.
            let is_ctor = !is_field && name == cls.fqn;
            cls.members.push(Member {
                name: if is_ctor { "new".to_string() } else { name },
                desc: desc.to_string(),
                is_static,
                is_ctor,
                is_field,
            });
            continue;
        }
        // A class/interface header.
        if (t.contains(" class ") || t.contains(" interface ") || t.starts_with("class ")
            || t.starts_with("interface "))
            && t.ends_with('{')
        {
            let kw = if t.contains(" class ") || t.starts_with("class ") {
                " class "
            } else {
                " interface "
            };
            let after = if let Some(p) = t.find(kw) {
                &t[p + kw.len()..]
            } else {
                t.split_whitespace().nth(1).unwrap_or("")
            };
            let fqn = after
                .split_whitespace()
                .next()
                .unwrap_or("")
                .split('<')
                .next()
                .unwrap_or("")
                .to_string();
            if fqn.is_empty() {
                continue;
            }
            let simple = fqn.rsplit('.').next().unwrap_or(&fqn).replace('$', "");
            let super_fqn = t.find(" extends ").map(|p| {
                t[p + " extends ".len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .split('<')
                    .next()
                    .unwrap_or("")
                    .to_string()
            });
            classes.push(Class { fqn, simple, super_fqn, members: Vec::new() });
            pending = None;
            continue;
        }
        if t.ends_with(';') {
            pending = Some((t.to_string(), t.contains(" static ")));
        }
    }
    classes
}

// ---- emitter ----------------------------------------------------------------

/// The `jni::JValue` expression for one argument.
fn arg_expr(t: &str, m: &Mapped, var: &str) -> String {
    if m.is_string {
        return format!("jni::arg_object({var}_s)");
    }
    match t {
        "Z" => format!("rt::arg_bool({var})"),
        "B" | "C" | "S" => format!("jni::arg_int({var} as i32)"),
        "I" => format!("jni::arg_int({var})"),
        "J" => format!("jni::arg_long({var})"),
        "F" => format!("jni::arg_float({var})"),
        "D" => format!("jni::arg_double({var})"),
        _ => format!("{}({var})", m.arg_ctor),
    }
}

struct Emitter {
    out: String,
    emitted: usize,
    skips: usize,
}

impl Emitter {
    fn skip(&mut self, kind: &str, name: &str, reason: &str) {
        self.skips += 1;
        self.out
            .push_str(&format!("    // SKIPPED {kind} `{name}`: {reason}\n"));
    }

    /// One method or constructor.
    fn emit_call(&mut self, cls: &Class, m: &Member, fname: &str) {
        let jni_cls = jni_name(&cls.fqn);
        let (params, ret) = match split_descriptor(&m.desc) {
            Some(p) => p,
            None => {
                self.skip("method", &m.name, "descriptor could not be parsed");
                return;
            }
        };
        let mut mapped = Vec::new();
        for p in &params {
            match map_type(p) {
                Some(mm) => mapped.push((p.clone(), mm)),
                None => {
                    self.skip("method", &m.name, &format!("parameter type `{p}` is not modelled"));
                    return;
                }
            }
        }
        let rmap = match map_type(&ret) {
            Some(r) => r,
            None => {
                self.skip("method", &m.name, &format!("return type `{ret}` is not modelled"));
                return;
            }
        };

        // signature
        let mut sig = String::new();
        if m.is_ctor || m.is_static {
            sig.push_str("env: rt::Env");
        } else {
            sig.push_str("this");
        }
        for (i, (_, mm)) in mapped.iter().enumerate() {
            sig.push_str(&format!(", arg{i}: {}", mm.cplus));
        }
        let ret_ty = if m.is_ctor {
            format!(" -> {}", cls.simple)
        } else if ret == "V" {
            String::new()
        } else if ret == "Z" {
            " -> bool".to_string()
        } else {
            format!(" -> {}", rmap.cplus)
        };
        self.out.push_str(&format!("    fn {fname}({sig}){ret_ty} {{\n"));

        let envx = if m.is_ctor || m.is_static { "env" } else { "this._env" };

        // jstrings for String/CharSequence parameters
        for (i, (_, mm)) in mapped.iter().enumerate() {
            if mm.is_string {
                self.out.push_str(&format!(
                    "        let arg{i}_s: jni::jstring = {envx}.new_string_utf(arg{i});\n"
                ));
            }
        }

        let mname = if m.is_ctor { "<init>" } else { &m.name };
        self.out.push_str(&format!(
            "        let mid: jni::jmethodID = {envx}.method(#str_ptr(\"{jni_cls}\\0\"), #str_ptr(\"{mname}\\0\"), #str_ptr(\"{}\\0\"));\n",
            m.desc
        ));

        let argp = if mapped.is_empty() {
            "0 as *jni::JValue".to_string()
        } else {
            let vals: Vec<String> = mapped
                .iter()
                .enumerate()
                .map(|(i, (t, mm))| arg_expr(t, mm, &format!("arg{i}")))
                .collect();
            self.out.push_str(&format!(
                "        var args: [jni::JValue; {}] = [{}];\n",
                mapped.len(),
                vals.join(", ")
            ));
            "#addr_of(args[0])".to_string()
        };

        let raw = format!("{envx}.raw()");
        if m.is_ctor {
            self.out.push_str(&format!(
                "        let cls: jni::jclass = {envx}.class_cached(#str_ptr(\"{jni_cls}\\0\"));\n"
            ));
            self.out.push_str(&format!(
                "        let obj: jni::jobject = {{ (*(*{raw})).NewObjectA({raw}, cls, mid, {argp}) }};\n"
            ));
        } else if m.is_static {
            self.out.push_str(&format!(
                "        let cls: jni::jclass = {envx}.class_cached(#str_ptr(\"{jni_cls}\\0\"));\n"
            ));
            let call = format!("CallStatic{}MethodA", rmap.call_kind);
            if ret == "V" {
                self.out.push_str(&format!(
                    "        {{ (*(*{raw})).{call}({raw}, cls, mid, {argp}); }};\n"
                ));
            } else {
                self.out.push_str(&format!(
                    "        let r: {} = {{ (*(*{raw})).{call}({raw}, cls, mid, {argp}) }};\n",
                    if ret == "Z" { "jni::jboolean".to_string() } else { rmap.cplus.clone() }
                ));
            }
        } else {
            let call = format!("Call{}MethodA", rmap.call_kind);
            if ret == "V" {
                self.out.push_str(&format!(
                    "        {{ (*(*{raw})).{call}({raw}, this._obj, mid, {argp}); }};\n"
                ));
            } else {
                self.out.push_str(&format!(
                    "        let r: {} = {{ (*(*{raw})).{call}({raw}, this._obj, mid, {argp}) }};\n",
                    if ret == "Z" { "jni::jboolean".to_string() } else { rmap.cplus.clone() }
                ));
            }
        }

        // release the temporary jstrings before returning
        for (i, (_, mm)) in mapped.iter().enumerate() {
            if mm.is_string {
                self.out
                    .push_str(&format!("        {envx}.delete_local_ref(arg{i}_s);\n"));
            }
        }

        if m.is_ctor {
            self.out.push_str(&format!(
                "        return {} {{ _env: env, _obj: env.retain_local_as_global(obj) }};\n",
                cls.simple
            ));
        } else if ret == "V" {
            self.out.push_str("        return;\n");
        } else if ret == "Z" {
            self.out.push_str("        return r as i8 != 0;\n");
        } else {
            self.out.push_str("        return r;\n");
        }
        self.out.push_str("    }\n\n");
        self.emitted += 1;
    }

    fn emit_field(&mut self, cls: &Class, m: &Member, fname: &str) {
        let jni_cls = jni_name(&cls.fqn);
        let fm = match map_type(&m.desc) {
            Some(f) => f,
            None => {
                self.skip("field", &m.name, &format!("type `{}` is not modelled", m.desc));
                return;
            }
        };
        if fm.is_string || m.desc.starts_with('L') || m.desc.starts_with('[') {
            self.skip("field", &m.name, "object fields are not bound; call the accessor instead");
            return;
        }
        let kind = fm.call_kind; // Int / Float / ... reused as the field-slot name
        let ret_ty = if m.desc == "Z" { "bool".to_string() } else { fm.cplus.clone() };
        if m.is_static {
            self.out
                .push_str(&format!("    fn {fname}(env: rt::Env) -> {ret_ty} {{\n"));
            self.out.push_str(&format!(
                "        let cls: jni::jclass = env.class_cached(#str_ptr(\"{jni_cls}\\0\"));\n"
            ));
            self.out.push_str(&format!(
                "        let fid: jni::jfieldID = env.static_field_id(cls, #str_ptr(\"{}\\0\"), #str_ptr(\"{}\\0\"));\n",
                m.name, m.desc
            ));
            self.out.push_str(&format!(
                "        let r: {} = {{ (*(*env.raw())).GetStatic{kind}Field(env.raw(), cls, fid) }};\n",
                if m.desc == "Z" { "jni::jboolean".to_string() } else { fm.cplus.clone() }
            ));
        } else {
            self.out
                .push_str(&format!("    fn {fname}(this) -> {ret_ty} {{\n"));
            self.out.push_str(&format!(
                "        let cls: jni::jclass = this._env.class_cached(#str_ptr(\"{jni_cls}\\0\"));\n"
            ));
            self.out.push_str(&format!(
                "        let fid: jni::jfieldID = this._env.field_id(cls, #str_ptr(\"{}\\0\"), #str_ptr(\"{}\\0\"));\n",
                m.name, m.desc
            ));
            self.out.push_str(&format!(
                "        let r: {} = {{ (*(*this._env.raw())).Get{kind}Field(this._env.raw(), this._obj, fid) }};\n",
                if m.desc == "Z" { "jni::jboolean".to_string() } else { fm.cplus.clone() }
            ));
        }
        if m.desc == "Z" {
            self.out.push_str("        return r as i8 != 0;\n");
        } else {
            self.out.push_str("        return r;\n");
        }
        self.out.push_str("    }\n\n");
        self.emitted += 1;
    }
}

/// Bind `classes` from `classpath` into one C+ module.
pub fn generate(classpath: &str, classes: &[String], runtime: &str) -> Result<String, String> {
    let parsed = read_classes(classpath, classes)?;
    Ok(emit_with_runtime(&parsed, runtime))
}

pub fn emit(parsed: &[Class]) -> String {
    emit_with_runtime(parsed, "android_view/runtime")
}

/// `runtime` is the import path for `rt::Env`. Generating INTO `android_view`
/// itself needs `./runtime`; generating a consumer package needs the package
/// path. Getting this wrong is a resolver error, not a silent bug, but it is
/// one nobody should have to hand-edit out of a 9,000-line file.
pub fn emit_with_runtime(parsed: &[Class], runtime: &str) -> String {
    let known: HashMap<String, String> = parsed
        .iter()
        .map(|c| (c.fqn.clone(), c.simple.clone()))
        .collect();

    let mut e = Emitter { out: String::new(), emitted: 0, skips: 0 };
    e.out.push_str(
        "// GENERATED by cpc-bindgen --java — DO NOT EDIT.\n\
         // Hand additions belong beside this file, not in it.\n\
         //\n\
         // Wrappers own a JNI GLOBAL ref, so an instance stays valid after the\n\
         // native call that made it returns. `into_raw` hands the ref out and\n\
         // disarms the drop.\n\
         //\n\
         // Every call goes through `rt::Env::method`, which caches the class and\n\
         // the method id: resolving per call measured a clean 2x slower on a\n\
         // 400-node mount (plans/plan.android.md rung 1).\n\
         //\n\
         // OVERLOADS: a name declared once keeps it. A name with more than one\n\
         // overload gets a parameter-type suffix on EVERY one of them, so a\n\
         // binding never silently changes meaning when the SDK adds an overload.\n\n\
         import \"jni/jni\" as jni;\n",
    );
    e.out.push_str(&format!("import \"{runtime}\" as rt;\n\n"));

    for c in parsed {
        e.out.push_str(&format!("// {}\n", c.fqn));
        e.out.push_str(&format!(
            "struct {} {{\n    _env: rt::Env,\n    _obj: jni::jobject,\n}}\n\n",
            c.simple
        ));
        e.out.push_str(&format!("impl {} {{\n", c.simple));
        e.out.push_str(&format!(
            "    fn drop(ref this) {{\n\
             \x20       // `into_raw` nulls `_obj`; DeleteGlobalRef does not admit NULL.\n\
             \x20       if this._obj != {{ 0 as jni::jobject }} {{ this._env.delete_global_ref(this._obj); }}\n\
             \x20       return;\n    }}\n\n\
             \x20   fn from_global(env: rt::Env, obj: jni::jobject) -> {n} {{ return {n} {{ _env: env, _obj: obj }}; }}\n\n\
             \x20   fn from_local(env: rt::Env, obj: jni::jobject) -> {n} {{ return {n} {{ _env: env, _obj: env.retain_local_as_global(obj) }}; }}\n\n\
             \x20   fn as_obj(this) -> jni::jobject {{ return this._obj; }}\n\n\
             \x20   fn into_raw(ref this) -> jni::jobject {{\n\
             \x20       let p: jni::jobject = this._obj;\n\
             \x20       this._obj = {{ 0 as jni::jobject }};\n\
             \x20       return p;\n    }}\n\n",
            n = c.simple
        ));

        // Typed upcast, when the superclass is part of this same binding.
        if let Some(sup) = &c.super_fqn {
            if let Some(sname) = known.get(sup) {
                if sname != &c.simple {
                    e.out.push_str(&format!(
                        "    // javap lists DECLARED members only; the inherited surface is here.\n\
                         \x20   //\n\
                         \x20   // Takes a FRESH global ref rather than sharing this one. Both\n\
                         \x20   // wrappers own what they hold and both drops run, so handing the\n\
                         \x20   // same ref over would DeleteGlobalRef it twice.\n\
                         \x20   fn as_{}(this) -> {} {{ return {}::from_global(this._env, this._env.new_global_ref(this._obj)); }}\n\n",
                        snake(sname),
                        sname,
                        sname
                    ));
                    e.emitted += 1;
                }
            }
        }

        // Group members so overloads can be disambiguated deterministically.
        let mut groups: HashMap<(String, bool, bool), Vec<&Member>> = HashMap::new();
        for m in &c.members {
            if !m.is_ctor && !is_ident(&m.name) {
                e.skip("member", &m.name, "not a Java identifier (static initializer or synthetic)");
                continue;
            }
            let base = if m.is_ctor { "new".to_string() } else { snake(&m.name) };
            groups
                .entry((base, m.is_static, m.is_field))
                .or_default()
                .push(m);
        }
        let mut keys: Vec<_> = groups.keys().cloned().collect();
        keys.sort();
        let mut used: Vec<String> = Vec::new();
        for k in keys {
            let ms = &groups[&k];
            for m in ms.iter() {
                let mut fname = k.0.clone();
                if ms.len() > 1 && !m.is_field {
                    if let Some((params, _)) = split_descriptor(&m.desc) {
                        let tags: Vec<String> = params
                            .iter()
                            .map(|p| map_type(p).map(|x| x.tag).unwrap_or_else(|| "x".into()))
                            .collect();
                        if tags.is_empty() {
                            fname.push_str("_void");
                        } else {
                            fname.push('_');
                            fname.push_str(&tags.join("_"));
                        }
                    }
                }
                // A field reader colliding with a method keeps both reachable.
                if used.contains(&fname) {
                    fname.push_str("_field");
                }
                if used.contains(&fname) {
                    continue;
                }
                used.push(fname.clone());
                if m.is_field {
                    e.emit_field(c, m, &fname);
                } else {
                    e.emit_call(c, m, &fname);
                }
            }
        }
        e.out.push_str("}\n\n");
    }

    e.out.push_str(&format!(
        "// cpc-bindgen --java: {} items emitted, {} SKIPPED.\n",
        e.emitted, e.skips
    ));
    e.out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"Compiled from "TextView.java"
public class android.widget.TextView extends android.view.View implements android.view.ViewTreeObserver$OnPreDrawListener {
  public static final int AUTO_SIZE_TEXT_TYPE_NONE;
    descriptor: I
  public android.widget.TextView(android.content.Context);
    descriptor: (Landroid/content/Context;)V
  public final void setText(java.lang.CharSequence);
    descriptor: (Ljava/lang/CharSequence;)V
  public final void setText(int);
    descriptor: (I)V
  public void setTextSize(float);
    descriptor: (F)V
  public boolean isFocused();
    descriptor: ()Z
  static {};
    descriptor: ()V
}
"#;

    #[test]
    fn snake_handles_camel_and_acronyms() {
        assert_eq!(snake("setTextSize"), "set_text_size");
        assert_eq!(snake("getX"), "get_x");
        assert_eq!(snake("setARGB"), "set_argb");
        assert_eq!(snake("isFocused"), "is_focused");
        // A nested class arrives as `TextView$BufferType`.
        assert_eq!(snake("TextView$BufferType"), "text_view_buffer_type");
    }

    #[test]
    fn descriptors_split_into_params_and_return() {
        assert_eq!(
            split_descriptor("(Ljava/lang/String;I)V").unwrap(),
            (vec!["Ljava/lang/String;".to_string(), "I".to_string()], "V".to_string())
        );
        assert_eq!(split_descriptor("()Z").unwrap(), (vec![], "Z".to_string()));
        // Arrays keep their rank, including arrays of objects.
        assert_eq!(
            split_descriptor("([CII)V").unwrap().0,
            vec!["[C".to_string(), "I".to_string(), "I".to_string()]
        );
        assert_eq!(
            split_descriptor("([[Ljava/lang/String;)V").unwrap().0,
            vec!["[[Ljava/lang/String;".to_string()]
        );
    }

    #[test]
    fn malformed_descriptors_are_rejected_not_guessed() {
        // Negative cases: every one of these would otherwise emit wrong code.
        assert!(split_descriptor("V").is_none()); // no parameter list
        assert!(split_descriptor("(Ljava/lang/String").is_none()); // unterminated class
        assert!(split_descriptor("(II").is_none()); // no closing paren
    }

    #[test]
    fn primitives_and_strings_map_to_their_jni_shapes() {
        assert_eq!(map_type("I").unwrap().cplus, "i32");
        assert_eq!(map_type("J").unwrap().cplus, "i64");
        assert_eq!(map_type("F").unwrap().cplus, "f32");
        assert_eq!(map_type("D").unwrap().cplus, "f64");
        assert_eq!(map_type("Z").unwrap().cplus, "bool");
        // A String parameter is taken as NUL-terminated UTF-8 and turned into a
        // jstring at the call site.
        let s = map_type("Ljava/lang/String;").unwrap();
        assert_eq!(s.cplus, "*u8");
        assert!(s.is_string);
        // CharSequence is the same deal — it is what setText actually takes.
        assert!(map_type("Ljava/lang/CharSequence;").unwrap().is_string);
        // Any other object is an opaque jobject, never invented as a wrapper.
        let o = map_type("Landroid/content/Context;").unwrap();
        assert_eq!(o.cplus, "jni::jobject");
        assert!(!o.is_string);
        assert_eq!(o.tag, "context");
    }

    #[test]
    fn an_unknown_type_letter_is_refused() {
        // Negative: `Q` is not a JVM type tag. Returning None makes the caller
        // emit `// SKIPPED`, which is the house rule.
        assert!(map_type("Q").is_none());
        assert!(map_type("").is_none());
    }

    #[test]
    fn javap_output_parses_into_members() {
        let cs = parse_javap(FIXTURE);
        assert_eq!(cs.len(), 1);
        let c = &cs[0];
        assert_eq!(c.fqn, "android.widget.TextView");
        assert_eq!(c.simple, "TextView");
        assert_eq!(c.super_fqn.as_deref(), Some("android.view.View"));
        assert!(c.members.iter().any(|m| m.is_ctor && m.desc == "(Landroid/content/Context;)V"));
        assert!(c.members.iter().any(|m| m.name == "setTextSize" && m.desc == "(F)V"));
        let f = c.members.iter().find(|m| m.name == "AUTO_SIZE_TEXT_TYPE_NONE").unwrap();
        assert!(f.is_field && f.is_static);
    }

    #[test]
    fn overloads_all_get_a_suffix_and_singletons_do_not() {
        let out = emit(&parse_javap(FIXTURE));
        // setText is overloaded, so BOTH get a tag — a later SDK adding a third
        // overload cannot silently change what `set_text` means.
        assert!(out.contains("fn set_text_str(this, arg0: *u8)"));
        assert!(out.contains("fn set_text_i(this, arg0: i32)"));
        assert!(!out.contains("fn set_text(this"));
        // setTextSize is declared once here, so it keeps the plain name.
        assert!(out.contains("fn set_text_size(this, arg0: f32)"));
    }

    #[test]
    fn a_string_parameter_round_trips_through_a_jstring() {
        let out = emit(&parse_javap(FIXTURE));
        assert!(out.contains("let arg0_s: jni::jstring = this._env.new_string_utf(arg0);"));
        assert!(out.contains("this._env.delete_local_ref(arg0_s);"));
    }

    #[test]
    fn a_boolean_return_is_narrowed_to_bool() {
        let out = emit(&parse_javap(FIXTURE));
        assert!(out.contains("fn is_focused(this) -> bool"));
        assert!(out.contains("return r as i8 != 0;"));
    }

    #[test]
    fn calls_route_through_the_caching_resolver() {
        // Not cosmetic: emitting a bare FindClass+GetMethodID per call site
        // measured 2x slower on a 400-node mount (plan.android.md rung 1).
        let out = emit(&parse_javap(FIXTURE));
        assert!(out.contains("this._env.method(#str_ptr(\"android/widget/TextView\\0\")"));
        assert!(!out.contains("find_class(#str_ptr(\"android/widget/TextView"));
    }

    #[test]
    fn a_static_initializer_is_skipped_not_emitted() {
        // Negative: javap prints `static {};`, whose name parses out as `{}`.
        // Emitting it produced `fn {}(this) {` — a parse error 5000 lines into
        // a generated file.
        assert!(!is_ident("{}"));
        let out = emit(&parse_javap(FIXTURE));
        assert!(out.contains("// SKIPPED member `{}`"));
        assert!(!out.contains("fn {}("));
    }

    #[test]
    fn a_known_superclass_becomes_a_typed_upcast() {
        // javap lists DECLARED members only, so the inherited surface has to be
        // reachable some way; `as_view()` is it.
        let mut cs = parse_javap(FIXTURE);
        cs.push(Class {
            fqn: "android.view.View".into(),
            simple: "View".into(),
            super_fqn: None,
            members: vec![],
        });
        let out = emit(&cs);
        assert!(out.contains("fn as_view(this) -> View"));
        // The upcast must take a FRESH global ref: both wrappers own what they
        // hold and both drops run, so sharing one ref is a double free.
        assert!(out.contains("View::from_global(this._env, this._env.new_global_ref(this._obj))"));
        assert!(!out.contains("View::from_global(this._env, this._obj)"));
    }

    #[test]
    fn an_unknown_superclass_produces_no_upcast() {
        // Negative: binding TextView alone must not emit `as_view` naming a
        // type that is not in the module.
        let out = emit(&parse_javap(FIXTURE));
        assert!(!out.contains("fn as_view"));
    }

    #[test]
    fn jni_names_use_slashes() {
        assert_eq!(jni_name("android.widget.TextView"), "android/widget/TextView");
    }
}
