pub mod dense;
#[cfg(test)]
mod dense_reference;
mod error;
pub mod lexical;
pub mod tokenize;

pub use error::RetrievalError;
pub use lexical::{LexicalDoc, LexicalIndex, ScoredRow};
