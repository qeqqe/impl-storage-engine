use sqlparser::ast as sp;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser as SqlParser;

use super::ast::*;
use super::error::{ParseError, ParseResult};

pub struct Parser;

impl Parser {
    pub fn parse(sql: &str) -> ParseResult<Statement> {
        let sql = sql.trim();
        if sql.is_empty() {
            return Err(ParseError::EmptyQuery);
        }

        // special commands not supported by sqlparser
        let trimmed_sql = sql.trim_end_matches(';').trim();
        let upper = trimmed_sql.to_uppercase();
        if upper == "BEGIN" || upper == "BEGIN TRANSACTION" || upper == "START TRANSACTION" {
            return Ok(Statement::Begin);
        }
        if upper == "COMMIT" {
            return Ok(Statement::Commit);
        }
        if upper == "ROLLBACK" {
            return Ok(Statement::Rollback);
        }
        if upper == "SHOW TABLES" || upper == "SHOW TABLE" {
            return Ok(Statement::ShowTables);
        }
        if upper.starts_with("DESCRIBE ") || upper.starts_with("DESC ") {
            let table = trimmed_sql
                .split_whitespace()
                .nth(1)
                .ok_or_else(|| ParseError::MissingClause("table name".into()))?;
            return Ok(Statement::Describe(table.to_string()));
        }

        let dialect = GenericDialect {};
        let statements = SqlParser::parse_sql(&dialect, sql)?;

        if statements.is_empty() {
            return Err(ParseError::EmptyQuery);
        }
        if statements.len() > 1 {
            return Err(ParseError::MultipleStatements);
        }

        Self::convert_statement(&statements[0])
    }

    /// parse multiple SQL statements
    pub fn parse_multi(sql: &str) -> ParseResult<Vec<Statement>> {
        let dialect = GenericDialect {};
        let statements = SqlParser::parse_sql(&dialect, sql)?;
        statements.iter().map(Self::convert_statement).collect()
    }

    fn convert_statement(stmt: &sp::Statement) -> ParseResult<Statement> {
        match stmt {
            sp::Statement::CreateTable(create) => Self::convert_create_table(create),
            sp::Statement::Drop {
                object_type,
                names,
                if_exists,
                ..
            } => Self::convert_drop(object_type, names, *if_exists),
            sp::Statement::Query(query) => Self::convert_query(query),
            sp::Statement::Insert(insert) => Self::convert_insert(insert),
            sp::Statement::Update(update) => {
                Self::convert_update(&update.table, &update.assignments, &update.selection)
            }
            sp::Statement::Delete(delete) => Self::convert_delete(delete),
            sp::Statement::StartTransaction { .. } => Ok(Statement::Begin),
            sp::Statement::Commit { .. } => Ok(Statement::Commit),
            sp::Statement::Rollback { .. } => Ok(Statement::Rollback),
            sp::Statement::ShowTables { .. } => Ok(Statement::ShowTables),
            sp::Statement::ExplainTable { table_name, .. } => {
                let name = Self::extract_table_name(table_name)?;
                Ok(Statement::Describe(name))
            }
            other => Err(ParseError::UnsupportedStatement(format!("{:?}", other))),
        }
    }

    fn convert_create_table(create: &sp::CreateTable) -> ParseResult<Statement> {
        let name = Self::extract_table_name(&create.name)?;
        let mut columns = create
            .columns
            .iter()
            .map(Self::convert_column_def)
            .collect::<ParseResult<Vec<_>>>()?;

        for constraint in &create.constraints {
            match constraint {
                sp::TableConstraint::PrimaryKey(pk) => {
                    for col in &pk.columns {
                        if let sp::Expr::Identifier(ident) = &col.column.expr
                            && let Some(target) = columns
                                .iter_mut()
                                .find(|c| c.name.eq_ignore_ascii_case(&ident.value))
                            && !target.constraints.contains(&ColumnConstraint::PrimaryKey)
                        {
                            target.constraints.push(ColumnConstraint::PrimaryKey);
                        }
                    }
                }
                sp::TableConstraint::Unique(uq) => {
                    for col in &uq.columns {
                        if let sp::Expr::Identifier(ident) = &col.column.expr
                            && let Some(target) = columns
                                .iter_mut()
                                .find(|c| c.name.eq_ignore_ascii_case(&ident.value))
                            && !target.constraints.contains(&ColumnConstraint::Unique)
                        {
                            target.constraints.push(ColumnConstraint::Unique);
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(Statement::CreateTable(CreateTable {
            name,
            columns,
            if_not_exists: create.if_not_exists,
        }))
    }

    fn convert_column_def(col: &sp::ColumnDef) -> ParseResult<ColumnDef> {
        let data_type = Self::convert_data_type(&col.data_type)?;
        let constraints = col
            .options
            .iter()
            .filter_map(|opt| Self::convert_column_option(&opt.option).transpose())
            .collect::<ParseResult<Vec<_>>>()?;

        Ok(ColumnDef {
            name: col.name.value.clone(),
            data_type,
            constraints,
        })
    }

    fn convert_data_type(dt: &sp::DataType) -> ParseResult<SqlDataType> {
        match dt {
            sp::DataType::Text
            | sp::DataType::Varchar(_)
            | sp::DataType::CharVarying(_)
            | sp::DataType::Character(_)
            | sp::DataType::Char(_)
            | sp::DataType::String(_) => Ok(SqlDataType::Text),

            sp::DataType::Int(_)
            | sp::DataType::Integer(_)
            | sp::DataType::BigInt(_)
            | sp::DataType::SmallInt(_)
            | sp::DataType::TinyInt(_) => Ok(SqlDataType::Integer),

            sp::DataType::Float(_)
            | sp::DataType::Real
            | sp::DataType::Double(_)
            | sp::DataType::DoublePrecision
            | sp::DataType::Decimal(_)
            | sp::DataType::Numeric(_) => Ok(SqlDataType::Float),

            sp::DataType::Boolean | sp::DataType::Bool => Ok(SqlDataType::Boolean),

            sp::DataType::JSON | sp::DataType::JSONB => Ok(SqlDataType::Json),

            sp::DataType::Timestamp(_, _) | sp::DataType::Datetime(_) | sp::DataType::Date => {
                Ok(SqlDataType::Timestamp)
            }

            sp::DataType::Uuid => Ok(SqlDataType::Uuid),

            other => Err(ParseError::UnsupportedDataType(format!("{:?}", other))),
        }
    }

    fn convert_column_option(opt: &sp::ColumnOption) -> ParseResult<Option<ColumnConstraint>> {
        match opt {
            sp::ColumnOption::Null => Ok(None), // Nullable by default
            sp::ColumnOption::NotNull => Ok(Some(ColumnConstraint::NotNull)),
            sp::ColumnOption::Unique(..) => Ok(Some(ColumnConstraint::Unique)),
            sp::ColumnOption::PrimaryKey(..) => Ok(Some(ColumnConstraint::PrimaryKey)),
            sp::ColumnOption::Default(expr) => {
                let e = Self::convert_expr(expr)?;
                Ok(Some(ColumnConstraint::Default(e)))
            }
            _ => Ok(None), // Ignore other constraints for now
        }
    }

    fn convert_drop(
        object_type: &sp::ObjectType,
        names: &[sp::ObjectName],
        if_exists: bool,
    ) -> ParseResult<Statement> {
        match object_type {
            sp::ObjectType::Table => {
                if names.len() != 1 {
                    return Err(ParseError::UnsupportedStatement(
                        "DROP multiple tables not supported".into(),
                    ));
                }
                let name = Self::extract_table_name(&names[0])?;
                Ok(Statement::DropTable(DropTable { name, if_exists }))
            }
            other => Err(ParseError::UnsupportedStatement(format!(
                "DROP {:?} not supported",
                other
            ))),
        }
    }

    fn convert_query(query: &sp::Query) -> ParseResult<Statement> {
        let body = &query.body;
        let select = match body.as_ref() {
            sp::SetExpr::Select(s) => s,
            other => {
                return Err(ParseError::UnsupportedStatement(format!(
                    "Unsupported query type: {:?}",
                    other
                )));
            }
        };

        let from = if select.from.len() != 1 {
            return Err(ParseError::UnsupportedStatement(
                "Exactly one table in FROM required".into(),
            ));
        } else {
            Self::extract_table_ref(&select.from[0])?
        };

        // SELECT columns
        let columns = Self::convert_projection(&select.projection)?;

        let where_clause = select
            .selection
            .as_ref()
            .map(Self::convert_expr)
            .transpose()?;

        // GROUP BY
        let group_by = match &select.group_by {
            sp::GroupByExpr::Expressions(exprs, modifiers) => {
                if !modifiers.is_empty() {
                    return Err(ParseError::UnsupportedStatement(
                        "GROUP BY ROLLUP/CUBE/GROUPING SETS not supported".into(),
                    ));
                }
                exprs
                    .iter()
                    .map(Self::convert_expr)
                    .collect::<ParseResult<Vec<_>>>()?
            }
            sp::GroupByExpr::All(_) => {
                return Err(ParseError::UnsupportedStatement(
                    "GROUP BY ALL not supported".into(),
                ));
            }
        };

        // HAVING
        let having = select.having.as_ref().map(Self::convert_expr).transpose()?;

        let order_by = query
            .order_by
            .as_ref()
            .map(Self::extract_order_by_exprs)
            .transpose()?
            .unwrap_or_default();

        let (limit, offset) = match &query.limit_clause {
            Some(sp::LimitClause::LimitOffset { limit, offset, .. }) => (
                limit.as_ref().and_then(Self::expr_to_usize),
                offset.as_ref().and_then(|o| Self::expr_to_usize(&o.value)),
            ),
            Some(sp::LimitClause::OffsetCommaLimit { offset, limit }) => {
                (Self::expr_to_usize(limit), Self::expr_to_usize(offset))
            }
            None => (None, None),
        };

        Ok(Statement::Select(Select {
            columns,
            from,
            where_clause,
            group_by,
            having,
            order_by,
            limit,
            offset,
        }))
    }

    fn convert_projection(items: &[sp::SelectItem]) -> ParseResult<Vec<SelectColumn>> {
        items
            .iter()
            .map(|item| match item {
                sp::SelectItem::Wildcard(_) => Ok(SelectColumn::Wildcard),
                sp::SelectItem::UnnamedExpr(expr) => match expr {
                    sp::Expr::Identifier(ident) => Ok(SelectColumn::Column {
                        table: None,
                        name: ident.value.clone(),
                    }),
                    sp::Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                        [table, col] => Ok(SelectColumn::Column {
                            table: Some(table.value.clone()),
                            name: col.value.clone(),
                        }),
                        _ => Err(ParseError::UnsupportedExpression(format!(
                            "Unsupported identifier depth: {:?}",
                            parts
                        ))),
                    },
                    _ => {
                        let e = Self::convert_expr(expr)?;
                        Ok(SelectColumn::Expr {
                            expr: e,
                            alias: None,
                        })
                    }
                },
                sp::SelectItem::ExprWithAlias { expr, alias } => {
                    let e = Self::convert_expr(expr)?;
                    Ok(SelectColumn::Expr {
                        expr: e,
                        alias: Some(alias.value.clone()),
                    })
                }
                sp::SelectItem::ExprWithAliases { expr, aliases } => {
                    let e = Self::convert_expr(expr)?;
                    Ok(SelectColumn::Expr {
                        expr: e,
                        alias: aliases.first().map(|a| a.value.clone()),
                    })
                }
                sp::SelectItem::QualifiedWildcard(kind, _) => match kind {
                    sp::SelectItemQualifiedWildcardKind::ObjectName(name) => Ok(
                        SelectColumn::QualifiedWildcard(Self::extract_table_name(name)?),
                    ),
                    other => Err(ParseError::UnsupportedExpression(format!(
                        "Unsupported qualified wildcard: {:?}",
                        other
                    ))),
                },
            })
            .collect()
    }

    fn extract_order_by_exprs(ob: &sp::OrderBy) -> ParseResult<Vec<OrderBy>> {
        match &ob.kind {
            sp::OrderByKind::All(_) => Err(ParseError::UnsupportedStatement(
                "ORDER BY ALL not supported".into(),
            )),
            sp::OrderByKind::Expressions(exprs) => {
                exprs.iter().map(Self::convert_order_by_expr).collect()
            }
        }
    }

    fn convert_order_by_expr(expr: &sp::OrderByExpr) -> ParseResult<OrderBy> {
        let column = match &expr.expr {
            sp::Expr::Identifier(id) => id.value.clone(),
            sp::Expr::CompoundIdentifier(parts) => {
                parts.last().map(|p| p.value.clone()).unwrap_or_default()
            }
            other => {
                return Err(ParseError::UnsupportedExpression(format!(
                    "ORDER BY expression: {:?}",
                    other
                )));
            }
        };
        let ascending = !matches!(expr.options.sort, Some(sp::OrderBySort::Desc));
        Ok(OrderBy { column, ascending })
    }

    fn convert_insert(insert: &sp::Insert) -> ParseResult<Statement> {
        let table = Self::extract_table_from_object(&insert.table)?;

        let columns = if insert.columns.is_empty() {
            None
        } else {
            Some(
                insert
                    .columns
                    .iter()
                    .map(Self::extract_table_name)
                    .collect::<ParseResult<Vec<_>>>()?,
            )
        };

        let values = match insert.source.as_ref().map(|s| s.body.as_ref()) {
            Some(sp::SetExpr::Values(sp::Values { rows, .. })) => rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(Self::convert_expr)
                        .collect::<ParseResult<Vec<_>>>()
                })
                .collect::<ParseResult<Vec<_>>>()?,
            _ => {
                return Err(ParseError::UnsupportedStatement(
                    "INSERT ... SELECT not supported".into(),
                ));
            }
        };

        Ok(Statement::Insert(Insert {
            table,
            columns,
            values,
        }))
    }

    fn convert_update(
        table: &sp::TableWithJoins,
        assignments: &[sp::Assignment],
        selection: &Option<sp::Expr>,
    ) -> ParseResult<Statement> {
        let table_name = Self::extract_single_table(table)?;

        let assigns = assignments
            .iter()
            .map(|a| {
                let column = Self::extract_assignment_target(&a.target)?;
                let value = Self::convert_expr(&a.value)?;
                Ok(Assignment { column, value })
            })
            .collect::<ParseResult<Vec<_>>>()?;

        let where_clause = selection.as_ref().map(Self::convert_expr).transpose()?;

        Ok(Statement::Update(Update {
            table: table_name,
            assignments: assigns,
            where_clause,
        }))
    }

    fn extract_assignment_target(target: &sp::AssignmentTarget) -> ParseResult<String> {
        match target {
            sp::AssignmentTarget::ColumnName(parts) => {
                // ObjectName has .0 field which is Vec<ObjectNamePart>
                Ok(parts
                    .0
                    .iter()
                    .map(|p| {
                        p.as_ident()
                            .map(|id| id.value.clone())
                            .unwrap_or_else(|| p.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join("."))
            }
            sp::AssignmentTarget::Tuple(parts) => {
                // tuple contains Vec<ObjectName>
                Ok(parts
                    .iter()
                    .flat_map(|obj| obj.0.iter())
                    .map(|p| {
                        p.as_ident()
                            .map(|id| id.value.clone())
                            .unwrap_or_else(|| p.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join("."))
            }
        }
    }

    fn convert_delete(delete: &sp::Delete) -> ParseResult<Statement> {
        let from = &delete.from;
        let tables = match from {
            sp::FromTable::WithFromKeyword(tables) => tables,
            sp::FromTable::WithoutKeyword(tables) => tables,
        };

        if tables.len() != 1 {
            return Err(ParseError::UnsupportedStatement(
                "DELETE from multiple tables not supported".into(),
            ));
        }

        let table = Self::extract_single_table(&tables[0])?;
        let where_clause = delete
            .selection
            .as_ref()
            .map(Self::convert_expr)
            .transpose()?;

        Ok(Statement::Delete(Delete {
            table,
            where_clause,
        }))
    }

    fn convert_expr(expr: &sp::Expr) -> ParseResult<Expr> {
        match expr {
            sp::Expr::Identifier(id) => Ok(Expr::Column {
                table: None,
                name: id.value.clone(),
            }),

            sp::Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [table, col] => Ok(Expr::Column {
                    table: Some(table.value.clone()),
                    name: col.value.clone(),
                }),
                _ => Err(ParseError::UnsupportedExpression(format!(
                    "Unsupported identifier depth: {:?}",
                    parts
                ))),
            },

            sp::Expr::Value(v) => Ok(Expr::Literal(Self::convert_value(v)?)),

            sp::Expr::BinaryOp { left, op, right } => {
                let l = Self::convert_expr(left)?;
                let r = Self::convert_expr(right)?;
                let o = Self::convert_binary_op(op)?;
                Ok(Expr::BinaryOp {
                    left: Box::new(l),
                    op: o,
                    right: Box::new(r),
                })
            }

            sp::Expr::UnaryOp { op, expr } => {
                let e = Self::convert_expr(expr)?;
                let o = Self::convert_unary_op(op)?;
                Ok(Expr::UnaryOp {
                    op: o,
                    expr: Box::new(e),
                })
            }

            sp::Expr::IsNull(e) => {
                let inner = Self::convert_expr(e)?;
                Ok(Expr::IsNull {
                    expr: Box::new(inner),
                    negated: false,
                })
            }

            sp::Expr::IsNotNull(e) => {
                let inner = Self::convert_expr(e)?;
                Ok(Expr::IsNull {
                    expr: Box::new(inner),
                    negated: true,
                })
            }

            sp::Expr::InList {
                expr,
                list,
                negated,
            } => {
                let e = Self::convert_expr(expr)?;
                let items = list
                    .iter()
                    .map(Self::convert_expr)
                    .collect::<ParseResult<Vec<_>>>()?;
                Ok(Expr::InList {
                    expr: Box::new(e),
                    list: items,
                    negated: *negated,
                })
            }

            sp::Expr::Between {
                expr,
                low,
                high,
                negated,
            } => {
                let e = Self::convert_expr(expr)?;
                let l = Self::convert_expr(low)?;
                let h = Self::convert_expr(high)?;
                Ok(Expr::Between {
                    expr: Box::new(e),
                    low: Box::new(l),
                    high: Box::new(h),
                    negated: *negated,
                })
            }

            sp::Expr::Like {
                expr,
                pattern,
                negated,
                ..
            } => {
                let e = Self::convert_expr(expr)?;
                let pat = Self::extract_string_from_expr(pattern)?;
                Ok(Expr::Like {
                    expr: Box::new(e),
                    pattern: pat,
                    negated: *negated,
                })
            }

            sp::Expr::Function(f) => {
                let name = f.name.to_string();
                let args = match &f.args {
                    sp::FunctionArguments::List(list) => list
                        .args
                        .iter()
                        .filter_map(|arg| match arg {
                            sp::FunctionArg::Unnamed(sp::FunctionArgExpr::Expr(e)) => {
                                Some(Self::convert_expr(e))
                            }
                            _ => None,
                        })
                        .collect::<ParseResult<Vec<_>>>()?,
                    _ => vec![],
                };
                Ok(Expr::Function { name, args })
            }

            sp::Expr::Nested(inner) => {
                let e = Self::convert_expr(inner)?;
                Ok(Expr::Nested(Box::new(e)))
            }

            other => Err(ParseError::UnsupportedExpression(format!("{:?}", other))),
        }
    }

    fn convert_value(v: &sp::ValueWithSpan) -> ParseResult<LiteralValue> {
        match &v.value {
            sp::Value::Null => Ok(LiteralValue::Null),
            sp::Value::Boolean(b) => Ok(LiteralValue::Boolean(*b)),
            sp::Value::Number(s, _) => {
                if let Ok(i) = s.parse::<i64>() {
                    Ok(LiteralValue::Integer(i))
                } else if let Ok(f) = s.parse::<f64>() {
                    Ok(LiteralValue::Float(f))
                } else {
                    Err(ParseError::UnsupportedExpression(format!(
                        "Invalid number: {}",
                        s
                    )))
                }
            }
            sp::Value::SingleQuotedString(s) => Ok(LiteralValue::String(s.clone())),
            sp::Value::DoubleQuotedString(s) => Ok(LiteralValue::String(s.clone())),
            other => Err(ParseError::UnsupportedExpression(format!(
                "Unsupported value: {:?}",
                other
            ))),
        }
    }

    fn extract_string_from_expr(expr: &sp::Expr) -> ParseResult<String> {
        match expr {
            sp::Expr::Value(v) => match &v.value {
                sp::Value::SingleQuotedString(s) => Ok(s.clone()),
                sp::Value::DoubleQuotedString(s) => Ok(s.clone()),
                _ => Err(ParseError::UnsupportedExpression("expected string".into())),
            },
            _ => Err(ParseError::UnsupportedExpression(
                "expected string literal".into(),
            )),
        }
    }

    fn convert_binary_op(op: &sp::BinaryOperator) -> ParseResult<BinaryOperator> {
        match op {
            sp::BinaryOperator::Eq => Ok(BinaryOperator::Eq),
            sp::BinaryOperator::NotEq => Ok(BinaryOperator::NotEq),
            sp::BinaryOperator::Lt => Ok(BinaryOperator::Lt),
            sp::BinaryOperator::LtEq => Ok(BinaryOperator::LtEq),
            sp::BinaryOperator::Gt => Ok(BinaryOperator::Gt),
            sp::BinaryOperator::GtEq => Ok(BinaryOperator::GtEq),
            sp::BinaryOperator::And => Ok(BinaryOperator::And),
            sp::BinaryOperator::Or => Ok(BinaryOperator::Or),
            sp::BinaryOperator::Plus => Ok(BinaryOperator::Plus),
            sp::BinaryOperator::Minus => Ok(BinaryOperator::Minus),
            sp::BinaryOperator::Multiply => Ok(BinaryOperator::Multiply),
            sp::BinaryOperator::Divide => Ok(BinaryOperator::Divide),
            sp::BinaryOperator::Modulo => Ok(BinaryOperator::Modulo),
            sp::BinaryOperator::StringConcat => Ok(BinaryOperator::Concat),
            other => Err(ParseError::UnsupportedExpression(format!(
                "Unsupported operator: {:?}",
                other
            ))),
        }
    }

    fn convert_unary_op(op: &sp::UnaryOperator) -> ParseResult<UnaryOperator> {
        match op {
            sp::UnaryOperator::Not => Ok(UnaryOperator::Not),
            sp::UnaryOperator::Minus => Ok(UnaryOperator::Minus),
            sp::UnaryOperator::Plus => Ok(UnaryOperator::Plus),
            other => Err(ParseError::UnsupportedExpression(format!(
                "Unsupported unary operator: {:?}",
                other
            ))),
        }
    }

    fn extract_table_name(name: &sp::ObjectName) -> ParseResult<String> {
        // just use the table name, ignore schema
        name.0
            .last()
            .map(|i| {
                i.as_ident()
                    .map(|id| id.value.clone())
                    .unwrap_or_else(|| i.to_string())
            })
            .ok_or_else(|| ParseError::InvalidIdentifier("empty table name".into()))
    }

    fn extract_table_from_object(table: &sp::TableObject) -> ParseResult<String> {
        match table {
            sp::TableObject::TableName(name) => Self::extract_table_name(name),
            sp::TableObject::TableFunction(_) => Err(ParseError::UnsupportedStatement(
                "table function not supported".into(),
            )),
            sp::TableObject::TableQuery(_) => Err(ParseError::UnsupportedStatement(
                "table query not supported".into(),
            )),
        }
    }

    fn extract_table_factor(factor: &sp::TableFactor) -> ParseResult<(String, Option<String>)> {
        match factor {
            sp::TableFactor::Table { name, alias, .. } => {
                let table = Self::extract_table_name(name)?;
                Ok((table, alias.as_ref().map(|a| a.name.value.clone())))
            }
            other => Err(ParseError::UnsupportedStatement(format!(
                "Unsupported table factor: {:?}",
                other
            ))),
        }
    }

    fn extract_table_ref(from: &sp::TableWithJoins) -> ParseResult<TableRef> {
        let (base, base_alias) = Self::extract_table_factor(&from.relation)?;
        let joins = from
            .joins
            .iter()
            .map(Self::convert_join)
            .collect::<ParseResult<Vec<_>>>()?;
        Ok(TableRef {
            base,
            base_alias,
            joins,
        })
    }

    fn convert_join(join: &sp::Join) -> ParseResult<Join> {
        let (table, alias) = Self::extract_table_factor(&join.relation)?;

        let (join_type, constraint) = match &join.join_operator {
            sp::JoinOperator::Join(c) | sp::JoinOperator::Inner(c) => (JoinType::Inner, c),
            sp::JoinOperator::Left(c) | sp::JoinOperator::LeftOuter(c) => (JoinType::Left, c),
            sp::JoinOperator::Right(c) | sp::JoinOperator::RightOuter(c) => (JoinType::Right, c),
            sp::JoinOperator::FullOuter(c) => (JoinType::Full, c),
            sp::JoinOperator::CrossJoin(_) => {
                return Ok(Join {
                    table,
                    alias,
                    join_type: JoinType::Cross,
                    on: None,
                });
            }
            other => {
                return Err(ParseError::UnsupportedStatement(format!(
                    "Unsupported join type: {:?}",
                    other
                )));
            }
        };

        let on = match constraint {
            sp::JoinConstraint::On(expr) => Some(Self::convert_expr(expr)?),
            sp::JoinConstraint::Using(_) => {
                return Err(ParseError::UnsupportedStatement(
                    "JOIN ... USING not supported, use ON".into(),
                ));
            }
            sp::JoinConstraint::Natural => {
                return Err(ParseError::UnsupportedStatement(
                    "NATURAL JOIN not supported".into(),
                ));
            }
            sp::JoinConstraint::None => {
                return Err(ParseError::MissingClause("JOIN ... ON condition".into()));
            }
        };

        Ok(Join {
            table,
            alias,
            join_type,
            on,
        })
    }

    fn extract_single_table(from: &sp::TableWithJoins) -> ParseResult<String> {
        if !from.joins.is_empty() {
            return Err(ParseError::UnsupportedStatement(
                "JOIN not supported in UPDATE/DELETE".into(),
            ));
        }
        Self::extract_table_factor(&from.relation).map(|(t, _)| t)
    }

    fn expr_to_usize(expr: &sp::Expr) -> Option<usize> {
        match expr {
            sp::Expr::Value(v) => match &v.value {
                sp::Value::Number(s, _) => s.parse().ok(),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_create_table() {
        let sql = "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, name TEXT NOT NULL, age INTEGER)";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::CreateTable(ct) => {
                assert_eq!(ct.name, "users");
                assert_eq!(ct.columns.len(), 3);
                assert!(ct.if_not_exists);

                assert_eq!(ct.columns[0].name, "id");
                assert!(
                    ct.columns[0]
                        .constraints
                        .contains(&ColumnConstraint::PrimaryKey)
                );

                assert_eq!(ct.columns[1].name, "name");
                assert!(
                    ct.columns[1]
                        .constraints
                        .contains(&ColumnConstraint::NotNull)
                );
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_parse_drop_table() {
        let sql = "DROP TABLE users";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::DropTable(dt) => {
                assert_eq!(dt.name, "users");
                assert!(!dt.if_exists);
            }
            _ => panic!("Expected DropTable"),
        }
    }

    #[test]
    fn test_parse_select_order_limit() {
        let sql = "SELECT * FROM users ORDER BY name DESC LIMIT 10 OFFSET 5";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.order_by.len(), 1);
                assert_eq!(s.order_by[0].column, "name");
                assert!(!s.order_by[0].ascending);
                assert_eq!(s.limit, Some(10));
                assert_eq!(s.offset, Some(5));
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_parse_complex_where() {
        let sql = "SELECT * FROM users WHERE age >= 18 AND (status = 'active' OR role = 'admin')";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Select(s) => {
                assert!(s.where_clause.is_some());
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_parse_insert() {
        let sql = "INSERT INTO users (id, name) VALUES ('1', 'Alice')";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Insert(i) => {
                assert_eq!(i.table, "users");
                assert_eq!(i.columns, Some(vec!["id".into(), "name".into()]));
                assert_eq!(i.values.len(), 1);
                assert_eq!(i.values[0].len(), 2);
            }
            _ => panic!("Expected Insert"),
        }
    }

    #[test]
    fn test_parse_update() {
        let sql = "UPDATE users SET name = 'Bob' WHERE id = '1'";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Update(u) => {
                assert_eq!(u.table, "users");
                assert_eq!(u.assignments.len(), 1);
                assert_eq!(u.assignments[0].column, "name");
                assert!(u.where_clause.is_some());
            }
            _ => panic!("Expected Update"),
        }
    }

    #[test]
    fn test_parse_delete() {
        let sql = "DELETE FROM users WHERE id = '1'";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Delete(d) => {
                assert_eq!(d.table, "users");
                assert!(d.where_clause.is_some());
            }
            _ => panic!("Expected Delete"),
        }
    }

    #[test]
    fn test_parse_describe() {
        match Parser::parse("DESCRIBE users").unwrap() {
            Statement::Describe(table) => assert_eq!(table, "users"),
            _ => panic!("Expected Describe"),
        }
    }

    #[test]
    fn test_parse_in_list() {
        let sql = "SELECT * FROM users WHERE status IN ('active', 'pending')";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Select(s) => match s.where_clause {
                Some(Expr::InList { list, negated, .. }) => {
                    assert_eq!(list.len(), 2);
                    assert!(!negated);
                }
                _ => panic!("Expected InList"),
            },
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_parse_like() {
        let sql = "SELECT * FROM users WHERE name LIKE 'A%'";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Select(s) => match s.where_clause {
                Some(Expr::Like {
                    pattern, negated, ..
                }) => {
                    assert_eq!(pattern, "A%");
                    assert!(!negated);
                }
                _ => panic!("Expected Like"),
            },
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_parse_between() {
        let sql = "SELECT * FROM users WHERE age BETWEEN 18 AND 65";
        let stmt = Parser::parse(sql).unwrap();

        match stmt {
            Statement::Select(s) => {
                assert!(matches!(
                    s.where_clause,
                    Some(Expr::Between { negated: false, .. })
                ));
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_empty_query() {
        assert!(matches!(
            Parser::parse("").unwrap_err(),
            ParseError::EmptyQuery
        ));
        assert!(matches!(
            Parser::parse("   ").unwrap_err(),
            ParseError::EmptyQuery
        ));
    }

    #[test]
    fn test_parse_multi() {
        let sql = "CREATE TABLE t1 (id INT); SELECT * FROM t1";
        let stmts = Parser::parse_multi(sql).unwrap();
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], Statement::CreateTable(_)));
        assert!(matches!(stmts[1], Statement::Select(_)));
    }

    #[test]
    fn test_table_level_constraints() {
        let sql = "CREATE TABLE users (id INT, email TEXT, PRIMARY KEY (id), UNIQUE (email))";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::CreateTable(ct) => {
                assert_eq!(ct.name, "users");
                let id_col = ct.columns.iter().find(|c| c.name == "id").unwrap();
                assert!(id_col.constraints.contains(&ColumnConstraint::PrimaryKey));
                let email_col = ct.columns.iter().find(|c| c.name == "email").unwrap();
                assert!(email_col.constraints.contains(&ColumnConstraint::Unique));
            }
            _ => panic!("Expected CreateTable"),
        }
    }

    #[test]
    fn test_qualified_column_in_expr() {
        let sql = "SELECT * FROM users WHERE users.age >= 18";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => match s.where_clause.unwrap() {
                Expr::BinaryOp { left, .. } => {
                    assert_eq!(
                        *left,
                        Expr::Column {
                            table: Some("users".into()),
                            name: "age".into(),
                        }
                    );
                }
                other => panic!("Expected BinaryOp, got {:?}", other),
            },
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_unqualified_column_in_expr() {
        let sql = "SELECT * FROM users WHERE age >= 18";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => match s.where_clause.unwrap() {
                Expr::BinaryOp { left, .. } => {
                    assert_eq!(
                        *left,
                        Expr::Column {
                            table: None,
                            name: "age".into(),
                        }
                    );
                }
                other => panic!("Expected BinaryOp, got {:?}", other),
            },
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_qualified_column_in_projection() {
        let sql = "SELECT a.id, b.name FROM a JOIN b ON a.id = b.a_id";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(
                    s.columns[0],
                    SelectColumn::Column {
                        table: Some("a".into()),
                        name: "id".into(),
                    }
                );
                assert_eq!(
                    s.columns[1],
                    SelectColumn::Column {
                        table: Some("b".into()),
                        name: "name".into(),
                    }
                );
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_unqualified_column_in_projection() {
        let sql = "SELECT id, name FROM users";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(
                    s.columns[0],
                    SelectColumn::Column {
                        table: None,
                        name: "id".into(),
                    }
                );
                assert_eq!(
                    s.columns[1],
                    SelectColumn::Column {
                        table: None,
                        name: "name".into(),
                    }
                );
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_inner_join() {
        let sql = "SELECT * FROM a JOIN b ON a.id = b.a_id";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.from.base, "a");
                assert!(s.from.base_alias.is_none());
                assert_eq!(s.from.joins.len(), 1);
                let j = &s.from.joins[0];
                assert_eq!(j.table, "b");
                assert_eq!(j.join_type, JoinType::Inner);
                match j.on.as_ref().unwrap() {
                    Expr::BinaryOp { left, op, right } => {
                        assert_eq!(
                            **left,
                            Expr::Column {
                                table: Some("a".into()),
                                name: "id".into(),
                            }
                        );
                        assert_eq!(*op, BinaryOperator::Eq);
                        assert_eq!(
                            **right,
                            Expr::Column {
                                table: Some("b".into()),
                                name: "a_id".into(),
                            }
                        );
                    }
                    other => panic!("Expected BinaryOp ON condition, got {:?}", other),
                }
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_left_join() {
        let sql = "SELECT * FROM a LEFT JOIN b ON a.id = b.a_id";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.from.joins.len(), 1);
                assert_eq!(s.from.joins[0].join_type, JoinType::Left);
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_multiple_joins() {
        let sql = "SELECT * FROM a JOIN b ON a.id = b.a_id LEFT JOIN c ON b.id = c.b_id";
        let stmt = Parser::parse(sql).unwrap();
        println!("{:#?}", stmt);
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.from.base, "a");
                assert_eq!(s.from.joins.len(), 2);
                assert_eq!(s.from.joins[0].table, "b");
                assert_eq!(s.from.joins[0].join_type, JoinType::Inner);
                assert_eq!(s.from.joins[1].table, "c");
                assert_eq!(s.from.joins[1].join_type, JoinType::Left);
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_table_alias() {
        let sql = "SELECT u.name FROM users AS u";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.from.base, "users");
                assert_eq!(s.from.base_alias.as_deref(), Some("u"));
                assert_eq!(
                    s.columns[0],
                    SelectColumn::Column {
                        table: Some("u".into()),
                        name: "name".into(),
                    }
                );
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_join_alias() {
        let sql = "SELECT * FROM a JOIN b AS bb ON a.id = bb.a_id";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.from.joins[0].table, "b");
                assert_eq!(s.from.joins[0].alias.as_deref(), Some("bb"));
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_cross_join() {
        let sql = "SELECT * FROM a CROSS JOIN b";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.from.joins.len(), 1);
                assert_eq!(s.from.joins[0].join_type, JoinType::Cross);
                assert!(s.from.joins[0].on.is_none());
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_group_by() {
        let sql = "SELECT name, COUNT(*) FROM users GROUP BY name";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.group_by.len(), 1);
                assert_eq!(
                    s.group_by[0],
                    Expr::Column {
                        table: None,
                        name: "name".into(),
                    }
                );
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_having() {
        let sql = "SELECT name, COUNT(*) FROM users GROUP BY name HAVING COUNT(*) > 1";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert!(s.having.is_some());
                assert_eq!(s.group_by.len(), 1);
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_no_group_by_produces_empty_vec() {
        let sql = "SELECT * FROM users";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert!(s.group_by.is_empty());
                assert!(s.having.is_none());
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_group_by_all_errors() {
        let sql = "SELECT name FROM users GROUP BY ALL";
        let result = Parser::parse(sql);
        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::UnsupportedStatement(msg) => {
                assert!(msg.contains("GROUP BY ALL"), "got: {}", msg);
            }
            other => panic!("Expected UnsupportedStatement, got {:?}", other),
        }
    }

    #[test]
    fn test_order_by_all_parsed_as_column() {
        let sql = "SELECT * FROM users ORDER BY ALL";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.order_by.len(), 1);
                // NOTE:`sqlparser::GenericDialect` treats ALL as a column name,
                // not OrderByKind::All The error path in extract_order_by_exprs
                // still guards against OrderByKind::All if a dialect that
                // supports it is ever used "ALL" parsed as a regular column name
                assert_eq!(s.order_by[0].column, "ALL");
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_qualified_wildcard() {
        let sql = "SELECT a.* FROM a JOIN b ON a.id = b.a_id";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(s.columns[0], SelectColumn::QualifiedWildcard("a".into()));
            }
            _ => panic!("Expected Select"),
        }
    }

    #[test]
    fn test_update_rejects_join() {
        let sql = "UPDATE a JOIN b ON a.id = b.a_id SET a.name = 'x'";
        let result = Parser::parse(sql);
        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::UnsupportedStatement(msg) => {
                assert!(msg.contains("JOIN"), "got: {}", msg);
            }
            other => panic!("Expected UnsupportedStatement, got {:?}", other),
        }
    }

    #[test]
    fn test_join_using_errors() {
        let sql = "SELECT * FROM a JOIN b USING (id)";
        let result = Parser::parse(sql);
        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::UnsupportedStatement(msg) => {
                assert!(msg.contains("USING"), "got: {}", msg);
            }
            other => panic!("Expected UnsupportedStatement, got {:?}", other),
        }
    }

    #[test]
    fn test_natural_join_errors() {
        let sql = "SELECT * FROM a NATURAL JOIN b";
        let result = Parser::parse(sql);
        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::UnsupportedStatement(msg) => {
                assert!(msg.contains("NATURAL"), "got: {}", msg);
            }
            other => panic!("Expected UnsupportedStatement, got {:?}", other),
        }
    }

    #[test]
    fn test_full_integration_query() {
        let sql = "SELECT a.id, b.name FROM a JOIN b ON a.id = b.a_id WHERE a.age >= 18 GROUP BY b.name HAVING COUNT(*) > 1";
        let stmt = Parser::parse(sql).unwrap();
        match stmt {
            Statement::Select(s) => {
                assert_eq!(
                    s.columns[0],
                    SelectColumn::Column {
                        table: Some("a".into()),
                        name: "id".into(),
                    }
                );
                assert_eq!(
                    s.columns[1],
                    SelectColumn::Column {
                        table: Some("b".into()),
                        name: "name".into(),
                    }
                );

                assert_eq!(s.from.base, "a");
                assert_eq!(s.from.joins.len(), 1);
                assert_eq!(s.from.joins[0].table, "b");
                assert_eq!(s.from.joins[0].join_type, JoinType::Inner);

                match s.from.joins[0].on.as_ref().unwrap() {
                    Expr::BinaryOp { left, op, right } => {
                        assert_eq!(
                            **left,
                            Expr::Column {
                                table: Some("a".into()),
                                name: "id".into(),
                            }
                        );
                        assert_eq!(*op, BinaryOperator::Eq);
                        assert_eq!(
                            **right,
                            Expr::Column {
                                table: Some("b".into()),
                                name: "a_id".into(),
                            }
                        );
                    }
                    other => panic!("Expected BinaryOp, got {:?}", other),
                }

                match s.where_clause.as_ref().unwrap() {
                    Expr::BinaryOp { left, op, right } => {
                        assert_eq!(
                            **left,
                            Expr::Column {
                                table: Some("a".into()),
                                name: "age".into(),
                            }
                        );
                        assert_eq!(*op, BinaryOperator::GtEq);
                        assert_eq!(**right, Expr::Literal(LiteralValue::Integer(18)));
                    }
                    other => panic!("Expected BinaryOp, got {:?}", other),
                }

                assert_eq!(s.group_by.len(), 1);
                assert_eq!(
                    s.group_by[0],
                    Expr::Column {
                        table: Some("b".into()),
                        name: "name".into(),
                    }
                );

                assert!(s.having.is_some());
                match s.having.as_ref().unwrap() {
                    Expr::BinaryOp { left, op, right } => {
                        assert_eq!(*op, BinaryOperator::Gt);
                        match left.as_ref() {
                            Expr::Function { name, .. } => {
                                assert_eq!(name.to_uppercase(), "COUNT");
                            }
                            other => panic!("Expected Function, got {:?}", other),
                        }
                        assert_eq!(**right, Expr::Literal(LiteralValue::Integer(1)));
                    }
                    other => panic!("Expected BinaryOp, got {:?}", other),
                }
            }
            _ => panic!("Expected Select"),
        }
    }
}
