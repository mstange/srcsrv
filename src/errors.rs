#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    InvalidUtf8,
    UnexpectedEof,
    UnrecognizedVersion,
    MissingVersion,
    MissingIniSection,
    MissingVariablesSection,
    MissingSrcSrvTrgField,
    MissingSourceFilesSection,
    MissingTerminationLine,
    MissingEquals,
    MissingPercent,
    MissingOpeningBracket,
    MissingClosingBracket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    NoFileMatch,
    Recursion,
    UnknownVariable,
}
