use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    pub properties: Vec<(String, String)>,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityParseError {
    pub offset: usize,
    pub message: &'static str,
}

impl fmt::Display for EntityParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for EntityParseError {}

pub fn parse_entities(input: &str) -> Result<Vec<Entity>, EntityParseError> {
    let mut parser = Parser {
        input: input.as_bytes(),
        offset: 0,
    };
    let mut entities = Vec::new();
    loop {
        parser.skip_space_and_comments()?;
        if parser.peek().is_none() {
            return Ok(entities);
        }
        parser.expect(b'{', "expected '{' to begin entity")?;
        let mut properties = Vec::new();
        loop {
            parser.skip_space_and_comments()?;
            match parser.peek() {
                Some(b'}') => {
                    parser.offset += 1;
                    break;
                }
                Some(b'"') => {
                    let key = parser.string()?;
                    parser.skip_space_and_comments()?;
                    let value = parser.string()?;
                    properties.push((key, value));
                }
                Some(_) => return Err(parser.error("expected quoted entity key or '}'")),
                None => return Err(parser.error("unterminated entity")),
            }
        }
        entities.push(Entity { properties });
    }
}

struct Parser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    fn error(&self, message: &'static str) -> EntityParseError {
        EntityParseError {
            offset: self.offset,
            message,
        }
    }

    fn expect(&mut self, byte: u8, message: &'static str) -> Result<(), EntityParseError> {
        if self.peek() == Some(byte) {
            self.offset += 1;
            Ok(())
        } else {
            Err(self.error(message))
        }
    }

    fn skip_space_and_comments(&mut self) -> Result<(), EntityParseError> {
        loop {
            while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
                self.offset += 1;
            }
            if self.input.get(self.offset..self.offset + 2) == Some(b"//") {
                self.offset += 2;
                while self.peek().is_some_and(|b| b != b'\n') {
                    self.offset += 1;
                }
                continue;
            }
            if self.input.get(self.offset..self.offset + 2) == Some(b"/*") {
                self.offset += 2;
                while self.offset + 1 < self.input.len()
                    && self.input.get(self.offset..self.offset + 2) != Some(b"*/")
                {
                    self.offset += 1;
                }
                if self.offset + 1 >= self.input.len() {
                    return Err(self.error("unterminated comment"));
                }
                self.offset += 2;
                continue;
            }
            return Ok(());
        }
    }

    fn string(&mut self) -> Result<String, EntityParseError> {
        self.expect(b'"', "expected quoted string")?;
        let mut value = Vec::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| self.error("unterminated quoted string"))?;
            self.offset += 1;
            match byte {
                b'"' => return Ok(String::from_utf8_lossy(&value).into_owned()),
                b'\\' => {
                    let escaped = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.offset += 1;
                    value.push(match escaped {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        other => other,
                    });
                }
                other => value.push(other),
            }
        }
    }
}
