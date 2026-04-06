/// AST node definitions for bash-compatible shell grammar.

#[derive(Debug, Clone, PartialEq)]
pub enum WordPart {
    Literal(String),
    SingleQuoted(String),
    DoubleQuoted(Vec<WordPart>),
    Variable(String),
    CommandSub(String),
    Glob(String),
    Tilde(String),
    BraceExpansion(Vec<Vec<WordPart>>),
    BraceRange { start: String, end: String, step: Option<String> },
    Arithmetic(String),
    ProcessSub(String, ProcessSubKind),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProcessSubKind {
    Input,  // <(cmd) -- provides input
    Output, // >(cmd) -- accepts output
}

pub type Word = Vec<WordPart>;

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
    pub index: Option<String>,       // For arr[idx]=value
    pub append: bool,                 // For var+=value or arr+=(...)
    pub array_value: Option<Vec<Word>>, // For arr=(a b c)
}

// Here-doc options to store multi-line content and modifiers
#[derive(Debug, Clone, PartialEq)]
pub struct HereDocOptions {
    pub delimiter: String,           // The delimiter (e.g., "EOF")
    pub content: String,             // The multi-line content
    pub strip_tabs: bool,            // <<- strips leading tabs
    pub expand_vars: bool,           // Whether to expand variables ($var, $(cmd), etc)
}

#[derive(Debug, Clone, PartialEq)]
pub enum RedirectTarget {
    File(Word),                      // Regular file redirection target
    HereDoc(HereDocOptions),         // Here-doc with content and options
}

#[derive(Debug, Clone, PartialEq)]
pub enum RedirectKind {
    Output,
    Append,
    Input,
    HereDoc,                         // << (still used for identification)
    HereString,
    DupOutput,
    DupInput,
    OutputAll,    // &> (redirect stdout and stderr)
    AppendAll,    // &>> (append stdout and stderr)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Redirect {
    pub fd: Option<i32>,
    pub kind: RedirectKind,
    pub target: Word,                // For backward compatibility, kept as Word
    pub here_doc: Option<HereDocOptions>, // Here-doc specific data
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: Vec<CompleteCommand>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompoundCommand {
    BraceGroup {
        body: Vec<CompleteCommand>,
        redirects: Vec<Redirect>,
    },
    Subshell {
        body: Vec<CompleteCommand>,
        redirects: Vec<Redirect>,
    },
    If {
        conditions: Vec<(Vec<CompleteCommand>, Vec<CompleteCommand>)>,
        else_branch: Option<Vec<CompleteCommand>>,
        redirects: Vec<Redirect>,
    },
    For {
        var: String,
        words: Option<Vec<Word>>,
        body: Vec<CompleteCommand>,
        redirects: Vec<Redirect>,
    },
    While {
        condition: Vec<CompleteCommand>,
        body: Vec<CompleteCommand>,
        redirects: Vec<Redirect>,
    },
    Until {
        condition: Vec<CompleteCommand>,
        body: Vec<CompleteCommand>,
        redirects: Vec<Redirect>,
    },
    Case {
        word: Word,
        arms: Vec<CaseArm>,
        redirects: Vec<Redirect>,
    },
    Select {
        var: String,
        words: Option<Vec<Word>>,
        body: Vec<CompleteCommand>,
        redirects: Vec<Redirect>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Simple(SimpleCommand),
    Compound(CompoundCommand),
    FunctionDef {
        name: String,
        body: Box<CompoundCommand>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pipeline {
    pub negated: bool,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Connector {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AndOrList {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompleteCommand {
    pub list: AndOrList,
    pub background: bool,
    pub disown: bool,
}
