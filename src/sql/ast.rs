#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    CreateTable(CreateTable),
    DropTable(DropTable),
    Select(Select),
    Insert(Insert),
    Update(Update),
    Delete(Delete),
    Begin,
    Commit,
    Rollback,
    ShowTables,
    Describe(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateTable {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: SqlDataType,
    pub constraints: Vec<ColumnConstraint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlDataType {
    Text,
    Integer,
    Float,
    Boolean,
    Json,
    Timestamp,
    Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnConstraint {
    NotNull,
    Unique,
    PrimaryKey,
    Default(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropTable {
    pub name: String,
    pub if_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    pub columns: Vec<SelectColumn>,
    pub from: TableRef,
    pub where_clause: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub order_by: Vec<OrderBy>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableRef {
    pub base: String,
    pub base_alias: Option<String>,
    pub joins: Vec<Join>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    pub table: String,
    pub alias: Option<String>,
    pub join_type: JoinType,
    pub on: Option<Expr>, // NOTE: None only possible on JoinType::Cross
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectColumn {
    Wildcard,
    QualifiedWildcard(String), // table.*
    Column { table: Option<String>, name: String },
    Expr { expr: Expr, alias: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderBy {
    pub column: String,
    pub ascending: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    pub table: String,
    pub columns: Option<Vec<String>>,
    pub values: Vec<Vec<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub table: String,
    pub assignments: Vec<Assignment>,
    pub where_clause: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub column: String,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delete {
    pub table: String,
    pub where_clause: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Column {
        table: Option<String>,
        name: String,
    },

    Literal(LiteralValue),

    BinaryOp {
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },

    UnaryOp {
        op: UnaryOperator,
        expr: Box<Expr>,
    },

    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },

    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },

    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },

    Like {
        expr: Box<Expr>,
        pattern: String,
        negated: bool,
    },

    Function {
        name: String,
        args: Vec<Expr>,
    },

    Nested(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
    Concat,
}

impl BinaryOperator {
    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            BinaryOperator::Eq
                | BinaryOperator::NotEq
                | BinaryOperator::Lt
                | BinaryOperator::LtEq
                | BinaryOperator::Gt
                | BinaryOperator::GtEq
        )
    }

    pub fn is_logical(&self) -> bool {
        matches!(self, BinaryOperator::And | BinaryOperator::Or)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Not,
    Minus,
    Plus,
}
