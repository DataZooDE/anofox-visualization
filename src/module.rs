//! Modules (#1156): features built in their own crates that plug into the
//! renderer at link time, without being part of this one.
//!
//! A module brings role tokens (`::MY_PANEL`) and draws the panels that
//! carry one of them. A distribution crate depends on anofox-visualization
//! and its modules and registers each once at start-up, before the first
//! render:
//!
//! ```ignore
//! anofox_visualization::module::register(&MY_MODULE)?;
//! ```
//!
//! The core knows nothing of what a module does: a module's tokens parse to
//! [`Role::Module`], and [`crate::render_with`] hands a panel carrying one
//! to the module. An unknown token stays a plain column, as before.
//!
//! **Unstable**: the interface may change without a semver bump until a
//! first real module exists.

use crate::roles::RoleSpec;
use crate::{Column, RenderOptions, Role};
use std::sync::RwLock;

/// One role a module brings.
#[derive(Clone, Copy, Debug)]
pub struct ModuleRole {
    /// The canonical token, upper case (`"MY_PANEL"`).
    pub token: &'static str,
    /// Accepted alternative spellings.
    pub aliases: &'static [&'static str],
    /// One line for `dashboard --roles` and the role lists.
    pub summary: &'static str,
}

/// A module: a name, the roles it brings, and how it draws a panel.
#[doc(hidden)]
pub trait Module: Send + Sync {
    /// A short, unique name (`"hello"`).
    fn name(&self) -> &'static str;
    /// The roles it brings; a statement carrying one draws a panel through
    /// [`Module::render`].
    fn roles(&self) -> &'static [ModuleRole];
    /// The panel's SVG body (what goes inside the `<svg>` of `width` ×
    /// `height`), from the statement's columns; an error is shown as the
    /// panel's error.
    fn render(
        &self,
        cols: &[Column],
        width: u32,
        height: u32,
        o: &RenderOptions,
    ) -> Result<String, String>;
}

struct Entry {
    module: &'static dyn Module,
    spec: &'static RoleSpec,
}

/// The registered roles, in registration order: `Role::Module(i)` is entry
/// `i`.
static REGISTRY: RwLock<Vec<Entry>> = RwLock::new(Vec::new());

/// Register `module` (once, at start-up). Refused when its name is taken or
/// one of its tokens or aliases is already a role (built in or a module's).
#[doc(hidden)]
pub fn register(module: &'static dyn Module) -> Result<(), String> {
    let mut reg = REGISTRY.write().map_err(|_| "module registry poisoned")?;
    if reg.iter().any(|e| e.module.name() == module.name()) {
        return Err(format!("module {} is registered already", module.name()));
    }
    let mut seen: Vec<&str> = vec![];
    for r in module.roles() {
        for t in std::iter::once(&r.token).chain(r.aliases) {
            let taken = crate::roles::REGISTRY.iter().any(|s| {
                s.token.eq_ignore_ascii_case(t)
                    || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(t))
            }) || reg.iter().any(|e| matches_spec(e.spec, t))
                || seen.iter().any(|s| s.eq_ignore_ascii_case(t));
            if taken {
                return Err(format!("module {}: ::{t} is a role already", module.name()));
            }
            seen.push(t);
        }
    }
    for r in module.roles() {
        let id = u16::try_from(reg.len()).map_err(|_| "too many module roles")?;
        // leaked once per role at start-up: role specs are `&'static`
        let spec: &'static RoleSpec = Box::leak(Box::new(RoleSpec {
            token: r.token,
            aliases: r.aliases,
            role: Role::Module(id),
            category: "module",
            renders: true,
            summary: r.summary,
        }));
        reg.push(Entry { module, spec });
    }
    Ok(())
}

fn matches_spec(s: &RoleSpec, t: &str) -> bool {
    s.token.eq_ignore_ascii_case(t) || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(t))
}

/// A module role by token or alias (case-insensitive).
pub(crate) fn lookup(token: &str) -> Option<&'static RoleSpec> {
    let t = token.trim();
    let reg = REGISTRY.read().ok()?;
    reg.iter().map(|e| e.spec).find(|s| matches_spec(s, t))
}

/// The spec of module role `id`.
pub(crate) fn spec(id: u16) -> Option<&'static RoleSpec> {
    REGISTRY.read().ok()?.get(id as usize).map(|e| e.spec)
}

/// Every registered module role, in registration order.
pub fn roles() -> Vec<&'static RoleSpec> {
    REGISTRY
        .read()
        .map(|r| r.iter().map(|e| e.spec).collect())
        .unwrap_or_default()
}

/// The names of the registered modules.
pub fn names() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = vec![];
    if let Ok(r) = REGISTRY.read() {
        for e in r.iter() {
            if !out.contains(&e.module.name()) {
                out.push(e.module.name());
            }
        }
    }
    out
}

/// The module that draws a panel with these columns: the first column with
/// a module role.
pub(crate) fn for_columns(cols: &[Column]) -> Option<&'static dyn Module> {
    let id = cols.iter().find_map(|c| match c.role {
        Role::Module(id) => Some(id),
        _ => None,
    })?;
    REGISTRY.read().ok()?.get(id as usize).map(|e| e.module)
}
