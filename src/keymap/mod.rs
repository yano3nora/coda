//! Keybinding parser, resolver, context predicates, conflict detection, and reports.

mod action;
mod binding;
mod context;
mod key_parse;
mod predicate;
mod resolver;
mod user_bindings;
mod vscode_commands;

pub use action::{EditorAction, ParseActionError};
pub use binding::{Binding, Source};
pub use context::EditorContext;
pub use key_parse::{ParseKeyError, parse_key_chord, parse_key_sequence};
pub use predicate::{ContextPredicate, ParsePredicateError};
pub use resolver::{ResolveResult, Resolver};
pub use user_bindings::{
    BindingIssue, BindingIssueReason, UserBindingsError, UserBindingsLoad, load_user_bindings,
};
pub use vscode_commands::action_for_vscode_command;
