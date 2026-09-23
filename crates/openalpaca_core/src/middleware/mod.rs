pub mod bootstrap;
pub(crate) mod body_sections;
pub mod guard;
pub mod identity;
mod persona_frontmatter;
pub mod prompt;
pub mod skill;
pub mod soul;
pub mod user;

#[cfg(test)]
mod body_sections_tests;
#[cfg(test)]
mod persona_frontmatter_tests;
