use core::fmt;

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
    pub primary_key: Vec<String>,
    pub unique_keys: Vec<Vec<String>>,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: SqlDataType,
    pub constraints: Vec<ColumnConstraint>,
}

impl ColumnDef {
    pub fn is_not_null(&self) -> bool {
        self.constraints
            .iter()
            .any(|c| matches!(c, ColumnConstraint::NotNull | ColumnConstraint::PrimaryKey))
    }

    pub fn default_value(&self) -> Option<&Expr> {
        self.constraints.iter().find_map(|cc| match cc {
            ColumnConstraint::Default(val) => Some(val),
            _ => None,
        })
    }
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
    pub distinct: bool,
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
    pub on: Option<Expr>,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

impl fmt::Display for JoinType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            JoinType::Inner => "Inner",
            JoinType::Left => "Left",
            JoinType::Right => "Right",
            JoinType::Full => "Full",
            JoinType::Cross => "Cross",
        };
        f.write_str(name)
    }
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
    pub expr: Expr,
    pub ascending: bool,
    pub nulls_first: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Insert {
    pub table: String,
    pub columns: Option<Vec<String>>,
    pub source: InsertSource,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertSource {
    Values(Vec<Vec<Expr>>),
    Select(Box<Select>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub table: String,
    pub alias: Option<String>,
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
    pub alias: Option<String>,
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
        case_insensitive: bool,
    },

    Function {
        name: String,
        args: FunctionArgs,
    },

    Nested(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArgs {
    Star,
    List { args: Vec<Expr>, distinct: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn is_arithmetic(&self) -> bool {
        matches!(
            self,
            BinaryOperator::Plus
                | BinaryOperator::Minus
                | BinaryOperator::Multiply
                | BinaryOperator::Divide
                | BinaryOperator::Modulo
        )
    }

    pub fn flip(&self) -> Option<BinaryOperator> {
        match self {
            BinaryOperator::Eq => Some(BinaryOperator::Eq),
            BinaryOperator::NotEq => Some(BinaryOperator::NotEq),
            BinaryOperator::Lt => Some(BinaryOperator::Gt),
            BinaryOperator::LtEq => Some(BinaryOperator::GtEq),
            BinaryOperator::Gt => Some(BinaryOperator::Lt),
            BinaryOperator::GtEq => Some(BinaryOperator::LtEq),
            _ => None,
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            BinaryOperator::Eq => "=",
            BinaryOperator::NotEq => "<>",
            BinaryOperator::Lt => "<",
            BinaryOperator::LtEq => "<=",
            BinaryOperator::Gt => ">",
            BinaryOperator::GtEq => ">=",
            BinaryOperator::And => "AND",
            BinaryOperator::Or => "OR",
            BinaryOperator::Plus => "+",
            BinaryOperator::Minus => "-",
            BinaryOperator::Multiply => "*",
            BinaryOperator::Divide => "/",
            BinaryOperator::Modulo => "%",
            BinaryOperator::Concat => "||",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOperator {
    Not,
    Minus,
    Plus,
}

impl UnaryOperator {
    pub fn symbol(&self) -> &'static str {
        match self {
            UnaryOperator::Not => "NOT ",
            UnaryOperator::Minus => "-",
            UnaryOperator::Plus => "+",
        }
    }
}

impl fmt::Display for LiteralValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LiteralValue::Null => f.write_str("NULL"),
            LiteralValue::Boolean(b) => f.write_str(if *b { "TRUE" } else { "FALSE" }),
            LiteralValue::Integer(i) => write!(f, "{i}"),
            LiteralValue::Float(x) => write!(f, "{x:?}"),
            LiteralValue::String(s) => write!(f, "'{}'", s.replace('\'', "''")),
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Column {
                table: Some(t),
                name,
            } => write!(f, "{t}.{name}"),
            Expr::Column { table: None, name } => f.write_str(name),
            Expr::Literal(lit) => write!(f, "{lit}"),
            Expr::BinaryOp { left, op, right } => write!(f, "{left} {} {right}", op.symbol()),
            Expr::UnaryOp { op, expr } => write!(f, "{}{expr}", op.symbol()),
            Expr::IsNull {
                expr,
                negated: false,
            } => write!(f, "{expr} IS NULL"),
            Expr::IsNull {
                expr,
                negated: true,
            } => write!(f, "{expr} IS NOT NULL"),
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let not = if *negated { "NOT " } else { "" };
                write!(f, "{expr} {not}IN (")?;
                write_list(f, list)?;
                f.write_str(")")
            }
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => {
                let not = if *negated { "NOT " } else { "" };
                write!(f, "{expr} {not}BETWEEN {low} AND {high}")
            }
            Expr::Like {
                expr,
                pattern,
                negated,
                case_insensitive,
            } => {
                let not = if *negated { "NOT " } else { "" };
                let kw = if *case_insensitive { "ILIKE" } else { "LIKE" };
                write!(
                    f,
                    "{expr} {not}{kw} {}",
                    LiteralValue::String(pattern.clone())
                )
            }
            Expr::Function { name, args } => match args {
                FunctionArgs::Star => write!(f, "{}(*)", name.to_uppercase()),
                FunctionArgs::List { args, distinct } => {
                    write!(f, "{}(", name.to_uppercase())?;
                    if *distinct {
                        f.write_str("DISTINCT ")?;
                    }
                    write_list(f, args)?;
                    f.write_str(")")
                }
            },
            Expr::Nested(inner) => write!(f, "({inner})"),
        }
    }
}

fn write_list(f: &mut fmt::Formatter<'_>, items: &[Expr]) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}
