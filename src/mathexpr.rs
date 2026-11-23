//! AE2's MathExpressionParser for its number fields (NumberEntryWidget): `+ - * / % ^` and brackets over decimal
//! numbers, so `2*64` or `(3+1)*16` can be typed where an amount goes. Upstream's `math_value`/`math_whole`
//! (`../ae2-horizon/mixins/ae2/me.rs`), with the input's length and nesting bounded (a client's field, never trusted
//! further: the server clamps whatever number it is sent).

/// The longest expression read.
const MOST_CHARS: usize = 64;
/// The deepest nesting of brackets and signs read.
const MOST_DEPTH: usize = 32;

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    depth: usize,
}

impl Parser<'_> {
    fn skip_spaces(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.chars.next();
        }
    }

    fn eat(&mut self, ch: char) -> bool {
        self.skip_spaces();
        if self.chars.peek() == Some(&ch) {
            self.chars.next();
            return true;
        }
        false
    }

    fn deeper(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MOST_DEPTH).then_some(())
    }

    fn sum(&mut self) -> Option<f64> {
        let mut v = self.product()?;
        loop {
            if self.eat('+') {
                v += self.product()?;
            } else if self.eat('-') {
                v -= self.product()?;
            } else {
                return Some(v);
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        loop {
            if self.eat('*') {
                v *= self.power()?;
            } else if self.eat('/') {
                let d = self.power()?;
                if d == 0.0 {
                    return None;
                }
                v /= d;
            } else if self.eat('%') {
                let d = self.power()?;
                if d == 0.0 {
                    return None;
                }
                v %= d;
            } else {
                return Some(v);
            }
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.eat('^') {
            self.deeper()?;
            let e = self.power()?;
            self.depth -= 1;
            Some(base.powf(e))
        } else {
            Some(base)
        }
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            self.deeper()?;
            let v = -self.unary()?;
            self.depth -= 1;
            return Some(v);
        }
        if self.eat('+') {
            self.deeper()?;
            let v = self.unary()?;
            self.depth -= 1;
            return Some(v);
        }
        if self.eat('(') {
            self.deeper()?;
            let v = self.sum()?;
            self.depth -= 1;
            return self.eat(')').then_some(v);
        }
        self.skip_spaces();
        let mut number = String::new();
        while let Some(c) = self.chars.peek().copied().filter(|c| c.is_ascii_digit() || *c == '.') {
            number.push(c);
            self.chars.next();
        }
        number.parse().ok()
    }
}

/// The expression's value; None for anything that isn't one (or divides by zero, or is too long or too deep).
pub(crate) fn value(text: &str) -> Option<f64> {
    if text.len() > MOST_CHARS || text.trim().is_empty() {
        return None;
    }
    let mut p = Parser { chars: text.chars().peekable(), depth: 0 };
    let v = p.sum()?;
    p.skip_spaces();
    (p.chars.peek().is_none() && v.is_finite()).then_some(v)
}

/// The expression as a whole number in `min..=max` (a plain number exactly, past what a float holds; an expression's
/// value rounded down, as AE2's fields take it). None when it isn't one.
pub(crate) fn whole(text: &str, min: i64, max: i64) -> Option<i64> {
    let t = text.trim();
    if let Ok(n) = t.parse::<i64>() {
        return Some(n.clamp(min, max));
    }
    let v = value(t)?.floor();
    // Beyond 2^53 a float no longer holds every whole number: clamped there.
    const EXACT: f64 = 9_007_199_254_740_992.0;
    Some((v.clamp(-EXACT, EXACT) as i64).clamp(min, max))
}

/// What a number field may take as typed: digits and the expression's signs.
pub(crate) fn allowed(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, '+' | '-' | '*' | '/' | '%' | '^' | '(' | ')' | '.' | ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_operators() {
        assert_eq!(value("2*64"), Some(128.0));
        assert_eq!(value(" 3 + 4 * 2 "), Some(11.0));
        assert_eq!(value("(3+1)*16"), Some(64.0));
        assert_eq!(value("10/4"), Some(2.5));
        assert_eq!(value("2^3^2"), Some(512.0));
        assert_eq!(value("-5+10"), Some(5.0));
        assert_eq!(value("7%4"), Some(3.0));
        assert_eq!(value("1.5*2"), Some(3.0));
    }

    #[test]
    fn rejects_what_isnt_an_expression() {
        assert_eq!(value(""), None);
        assert_eq!(value("2*"), None);
        assert_eq!(value("(2+3"), None);
        assert_eq!(value("2)"), None);
        assert_eq!(value("abc"), None);
        assert_eq!(value("1/0"), None);
        assert_eq!(value("5%0"), None);
        assert_eq!(value("1..2"), None);
        assert_eq!(value("10^400"), None);
    }

    #[test]
    fn bounded_input() {
        // Deep nesting and long input are refused, not overflowing the stack.
        let deep = format!("{}1{}", "(".repeat(1000), ")".repeat(1000));
        assert_eq!(value(&deep), None);
        assert_eq!(value(&"-".repeat(60).to_string()), None);
        assert_eq!(value(&"1+".repeat(40)), None);
        let nested = format!("{}1{}", "(".repeat(20), ")".repeat(20));
        assert_eq!(value(&nested), Some(1.0));
    }

    #[test]
    fn whole_numbers_clamped() {
        assert_eq!(whole("2*64", 1, i64::from(i32::MAX)), Some(128));
        assert_eq!(whole("0", 1, 100), Some(1));
        assert_eq!(whole("-3", 1, 100), Some(1));
        assert_eq!(whole("1000*1000*1000*1000", 1, i64::from(i32::MAX)), Some(i64::from(i32::MAX)));
        assert_eq!(whole("9/2", 0, 100), Some(4));
        assert_eq!(whole("12345678901234", 0, i64::MAX / 2), Some(12_345_678_901_234));
        assert_eq!(whole("2*", 0, 100), None);
    }
}
