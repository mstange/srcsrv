/// An enum for errors that occur during stream parsing.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    #[error("The srcsrv stream is not valid utf-8.")]
    InvalidUtf8,

    #[error("The srcsrv stream ended unexpectedly.")]
    UnexpectedEof,

    #[error("Version {0} is not a recognized srcsrv stream version.")]
    UnrecognizedVersion(String),

    #[error("The VERSION ini variable is missing.")]
    MissingVersion,

    #[error("Could not find the ini section in the srcsrv stream.")]
    MissingIniSection,

    #[error("Could not find the variables section in the srcsrv stream.")]
    MissingVariablesSection,

    #[error("The SRCSRVTRG field was missing. This is a required field.")]
    MissingSrcSrvTrgField,

    #[error("Could not find the source files section in the srcsrv stream.")]
    MissingSourceFilesSection,

    #[error("Could not find the end marker line in theh srcsrv stream.")]
    MissingTerminationLine,

    #[error("Missing = in a variable line in the srcsrv stream.")]
    MissingEquals,

    #[error("Missing closing % in srcsrv variable use.")]
    MissingPercent,

    #[error("Expected ( after {0} function in srcsrv variable.")]
    MissingOpeningParen(String),

    #[error("Could not find closing ) for {0} function in srcsrv variable.")]
    MissingClosingParen(String),
}

/// An enum for errors that can occur when looking up the SourceRetrievalMethod
/// for a file, and when evaluating the variables.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EvalError {
    #[error("Encountered recursion while evaluating srcsrv variable {0}.")]
    Recursion(String),

    #[error("Could not resolve srcsrv variable name {0}.")]
    UnknownVariable(String),
}
