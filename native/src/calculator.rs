//! Local arithmetic only: no scripting, network access, or implicit evaluation.
use rust_decimal::Decimal;

#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub value: Decimal,
    pub approximate: bool,
}
impl Answer {
    pub fn full(&self) -> String {
        self.value.normalize().to_string()
    }
    pub fn display(&self) -> String {
        let full = self.full();
        let rounded = self
            .value
            .round_sf_with_strategy(12, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
            .unwrap_or(self.value);
        // Keep tiny nonzero values readable rather than displaying a misleading zero.
        let value = if rounded.is_zero() && !self.value.is_zero() {
            full.clone()
        } else {
            rounded.normalize().to_string()
        };
        if self.approximate || value != full {
            format!("≈ {value}")
        } else {
            value
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalcError {
    pub message: String,
    pub position: usize,
    pub incomplete: bool,
}

pub fn expression_query(query: &str) -> Option<&str> {
    query.strip_prefix('=').or_else(|| query.strip_prefix('＝'))
}

pub fn evaluate(input: &str) -> Result<Answer, CalcError> {
    let chars: Vec<char> = input
        .chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap(),
            '＋' => '+',
            '−' | '－' => '-',
            '×' | '＊' => '*',
            '÷' | '／' => '/',
            '（' => '(',
            '）' => ')',
            '％' => '%',
            '．' => '.',
            other => other,
        })
        .collect();
    let mut p = Parser {
        chars,
        at: 0,
        depth: 0,
        approximate: false,
    };
    if p.chars.len() > 4096 {
        return Err(p.error("算式最多 4096 个字符", false));
    }
    if p.chars.iter().any(|c| matches!(c, '\n' | '\r')) {
        return Err(p.error("每次输入一个算式", false));
    }
    let value = p.sum()?;
    p.space();
    if p.at != p.chars.len() {
        let message = match p.chars[p.at] {
            '(' => "括号前请加乘号".into(),
            ',' | '，' => "请移除数字中的逗号".into(),
            ')' => "多了一个右括号".into(),
            '=' | '＝' => "不支持比较运算".into(),
            c => format!("不支持“{c}”，请检查算式"),
        };
        return Err(p.error(&message, false));
    }
    Ok(Answer {
        value,
        approximate: p.approximate,
    })
}

struct Parser {
    chars: Vec<char>,
    at: usize,
    depth: usize,
    approximate: bool,
}
impl Parser {
    fn error(&self, message: &str, incomplete: bool) -> CalcError {
        CalcError {
            message: message.into(),
            position: self.at,
            incomplete,
        }
    }
    fn space(&mut self) {
        while self.chars.get(self.at).is_some_and(|c| c.is_whitespace()) {
            self.at += 1;
        }
    }
    fn take(&mut self, c: char) -> bool {
        self.space();
        if self.chars.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn sum(&mut self) -> Result<Decimal, CalcError> {
        let mut v = self.product()?;
        loop {
            if self.take('+') {
                let r = self.product()?;
                let n = self.checked(v.checked_add(r))?;
                if n.checked_sub(v) != Some(r) || n.checked_sub(r) != Some(v) {
                    self.approximate = true;
                }
                v = n;
            } else if self.take('-') {
                let r = self.product()?;
                let n = self.checked(v.checked_sub(r))?;
                if n.checked_add(r) != Some(v) || v.checked_sub(n) != Some(r) {
                    self.approximate = true;
                }
                v = n;
            } else {
                return Ok(v);
            }
        }
    }
    fn product(&mut self) -> Result<Decimal, CalcError> {
        let mut v = self.unary()?;
        loop {
            if self.take('*') {
                let r = self.unary()?;
                let n = self.checked(v.checked_mul(r))?;
                if !v.is_zero() && !r.is_zero() && n.is_zero() {
                    return Err(self.error("结果超出支持范围", false));
                }
                if !r.is_zero() && n.checked_div(r) != Some(v) {
                    self.approximate = true;
                }
                v = n;
            } else if self.take('/') {
                let r = self.unary()?;
                if r.is_zero() {
                    return Err(self.error("不能除以 0", false));
                }
                let n = self.checked(v.checked_div(r))?;
                if !v.is_zero() && n.is_zero() {
                    return Err(self.error("结果超出支持范围", false));
                }
                if n.checked_mul(r) != Some(v) {
                    self.approximate = true;
                }
                v = n;
            } else {
                return Ok(v);
            }
        }
    }
    fn unary(&mut self) -> Result<Decimal, CalcError> {
        // Iterative signs keep even an adversarial 4096-character input stack-safe.
        let mut negative = false;
        loop {
            if self.take('-') {
                negative = !negative;
            } else if !self.take('+') {
                break;
            }
        }
        let mut v = self.atom()?;
        if self.take('%') {
            let next = self.checked(v.checked_div(Decimal::ONE_HUNDRED))?;
            if !v.is_zero() && next.is_zero() {
                return Err(self.error("结果超出支持范围", false));
            }
            if next.checked_mul(Decimal::ONE_HUNDRED) != Some(v) {
                self.approximate = true;
            }
            v = next;
        }
        Ok(if negative { -v } else { v })
    }
    fn atom(&mut self) -> Result<Decimal, CalcError> {
        if self.take('(') {
            self.depth += 1;
            if self.depth > 64 {
                return Err(self.error("括号最多嵌套 64 层", false));
            }
            let value = self.sum()?;
            if !self.take(')') {
                return Err(self.error("还差一个右括号", self.at == self.chars.len()));
            }
            self.depth -= 1;
            return Ok(value);
        }
        self.space();
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| c.is_ascii_digit() || *c == '.')
        {
            self.at += 1;
        }
        if start == self.at {
            return Err(self.error("请输入数字或括号", self.at == self.chars.len()));
        }
        if self
            .chars
            .get(self.at)
            .is_some_and(|c| matches!(c, 'e' | 'E'))
        {
            self.at += 1;
            if self
                .chars
                .get(self.at)
                .is_some_and(|c| matches!(c, '+' | '-'))
            {
                self.at += 1;
            }
            let exponent = self.at;
            while self.chars.get(self.at).is_some_and(|c| c.is_ascii_digit()) {
                self.at += 1;
            }
            if exponent == self.at {
                return Err(self.error("请补全科学计数法指数", self.at == self.chars.len()));
            }
        }
        let token: String = self.chars[start..self.at].iter().collect();
        let parsed = if token.contains(['e', 'E']) {
            Decimal::from_scientific(&token)
        } else {
            Decimal::from_str_exact(&token)
        };
        parsed.map_err(|_| self.error("数字格式有误或超出支持精度", token == "."))
    }
    fn checked(&self, value: Option<Decimal>) -> Result<Decimal, CalcError> {
        value.ok_or_else(|| self.error("结果超出支持范围", false))
    }
}

#[derive(Clone)]
pub struct Record {
    pub expression: String,
    pub answer: Answer,
}

#[derive(Default)]
pub struct Calculator {
    pub active: bool,
    pub expression: String,
    pub draft: String,
    pub confirmed: Option<Answer>,
    pub continuation_approximate: bool,
    pub history: Vec<Record>,
    pub focus: bool,
    pub show_full: bool,
    pub notice: Option<(String, std::time::Instant)>,
    pub removed: Option<(Vec<Record>, std::time::Instant)>,
}
impl Calculator {
    pub fn enter(&mut self, expression: &str) {
        self.active = true;
        self.expression = expression.into();
        self.confirmed = None;
        self.continuation_approximate = false;
        self.focus = true;
        self.show_full = false;
    }
    pub fn leave(&mut self) {
        self.draft = self.expression.clone();
        self.active = false;
        self.confirmed = None;
    }
    pub fn confirm(&mut self, answer: Answer) {
        if !self
            .history
            .first()
            .is_some_and(|r| r.expression == self.expression && r.answer == answer)
        {
            self.history.insert(
                0,
                Record {
                    expression: self.expression.clone(),
                    answer: answer.clone(),
                },
            );
            self.history.truncate(50);
        }
        self.confirmed = Some(answer);
        self.focus = true;
    }
    pub fn first_input(&mut self, text: &str, paste: bool) -> bool {
        let Some(answer) = self.confirmed.take() else {
            return false;
        };
        let continuation = !paste && text.starts_with(['+', '-', '*', '/', '×', '÷']);
        self.continuation_approximate = continuation && answer.approximate;
        self.expression = if continuation {
            format!("({})", answer.full())
        } else {
            String::new()
        };
        true
    }
    pub fn result(&self) -> Result<Answer, CalcError> {
        evaluate(&self.expression).map(|mut answer| {
            answer.approximate |= self.continuation_approximate;
            answer
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arithmetic_and_precedence() {
        for (expression, expected) in [
            ("128*3+56", "440"),
            ("0.1+0.2", "0.3"),
            ("200+10%", "200.1"),
            ("200*(1+10%)", "220"),
            ("-(-3)*2", "6"),
            ("１２０÷（２＋４）", "20"),
            ("1e-3*1000", "1"),
            (".5+0.50", "1"),
            ("8/4/2", "1"),
            ("1/8", "0.125"),
        ] {
            assert_eq!(
                evaluate(expression).unwrap().full(),
                expected,
                "{expression}"
            );
        }
        assert!(evaluate("1/3").unwrap().approximate);
        assert!(!evaluate("1/8").unwrap().approximate);
    }
    #[test]
    fn rejects_invalid_and_bounded_inputs() {
        for expression in [
            "1/0",
            "2(3+4)",
            "1,000",
            "1\n+2",
            "1=2",
            "3..4",
            "1e",
            "2%%",
            "999999999999999999999999999999999",
            "1e-28/10",
        ] {
            assert!(evaluate(expression).is_err(), "{expression}");
        }
        assert!(evaluate("12*").unwrap_err().incomplete);
        assert!(evaluate("(1+2").unwrap_err().incomplete);
        assert!(evaluate(&format!("{}1{}", "(".repeat(65), ")".repeat(65))).is_err());
        assert!(evaluate(&"1".repeat(4097)).is_err());
        assert_eq!(
            evaluate(&format!("{}1", "-".repeat(4000))).unwrap().full(),
            "1"
        );
    }
    #[test]
    fn mode_and_history() {
        assert_eq!(expression_query("＝1+2"), Some("1+2"));
        assert_eq!(expression_query("a=1"), None);
        assert_eq!(expression_query("\\=SUM"), None);
        let mut calc = Calculator::default();
        calc.enter("1+2");
        calc.confirm(evaluate(&calc.expression).unwrap());
        calc.confirm(evaluate(&calc.expression).unwrap());
        assert_eq!(calc.history.len(), 1);
        calc.leave();
        assert_eq!(calc.draft, "1+2");
        calc.enter("");
        assert!(calc.expression.is_empty());
        assert_eq!(calc.history.len(), 1);
        for i in 0..60 {
            calc.expression = i.to_string();
            calc.confirm(evaluate(&calc.expression).unwrap());
        }
        assert_eq!(calc.history.len(), 50);
    }
    #[test]
    fn continuation_uses_full_precision_and_paste_starts_fresh() {
        let mut calc = Calculator::default();
        calc.enter("1/3");
        calc.confirm(calc.result().unwrap());
        assert!(calc.first_input("*", false));
        calc.expression.push_str("*3");
        assert!(calc.result().unwrap().approximate);
        assert!(calc
            .expression
            .starts_with("(0.3333333333333333333333333333)"));
        calc.confirm(calc.result().unwrap());
        calc.first_input("-5", true);
        assert!(calc.expression.is_empty());
        assert!(!calc.continuation_approximate);
        calc.expression.push_str("-5");
        assert_eq!(calc.result().unwrap().full(), "-5");
        assert!(!calc.first_input("2", false));
    }
}
