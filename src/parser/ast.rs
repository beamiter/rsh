/// AST node definitions for bash-compatible shell grammar.

#[derive(Debug, Clone)]
pub enum WordPart {
    Literal(String),
    SingleQuoted(String),
    DoubleQuoted(Vec<WordPart>),
    Variable(String),
    CommandSub(String),
    Glob(String),
    Tilde(String),
    BraceExpansion(Vec<Vec<WordPart>>),
    Arithmetic(String),
}

pub type Word = Vec<WordPart>;

#[derive(Debug, Clone)]
pub struct Assignment {
    pub name: String,
    pub value: Word,
}

#[derive(Debug, Clone)]
pub enum RedirectKind {
    Output,
    Append,
    Input,
    HereDoc,
    HereString,
    DupOutput,
    DupInput,
}

#[derive(Debug, Clone)]
pub struct Redirect {
    pub fd: Option<i32>,
    pub kind: RedirectKind,
    pub target: Word,
}

#[derive(Debug, Clone)]
pub struct SimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: Vec<CompleteCommand>,
}

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
pub enum Command {
    Simple(SimpleCommand),
    Compound(CompoundCommand),
    FunctionDef {
        name: String,
        body: Box<CompoundCommand>,
    },
}

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub negated: bool,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Connector {
    And,
    Or,
}

#[derive(Debug, Clone)]
pub struct AndOrList {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}

#[derive(Debug, Clone)]
pub struct CompleteCommand {
    pub list: AndOrList,
    pub background: bool,
}
