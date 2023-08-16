use anyhow::anyhow;

pub enum StackItem {
    Number(f64),
    Operation(Operation),
    ErrNum(String),
    ErrOp(String),
}

impl StackItem {
    pub fn get_f64(&self) -> anyhow::Result<f64> {
        match self {
            StackItem::Number(x) => Ok(x.to_owned()),
            _ => Err(anyhow::anyhow!("unwrap called on non-numeric value".to_string())),
        }
    }
}

pub enum Operation {
    Add,
    Multiply,
    Subtract,
    Divide,
}

pub struct Stack {
    pub stack: Vec<StackItem>,
}

impl Stack {
    pub fn init(input: &str) -> anyhow::Result<Stack> {
        let stack = input
            .trim()
            .split(" ")
            .map(|x| match x {
                "+" => StackItem::Operation(Operation::Add),
                "-" => StackItem::Operation(Operation::Subtract),
                "*" => StackItem::Operation(Operation::Multiply),
                "/" => StackItem::Operation(Operation::Divide),
                _ => {
                    return if let Ok(n) = x.parse::<f64>() {
                        StackItem::Number(n)
                    } else {
                        StackItem::ErrNum(x.to_string())
                    }
                },
            })
            .collect::<Vec<StackItem>>();

        let has_error = stack.iter().find(|&x| {
            match x {
                StackItem::ErrNum(_) | StackItem::ErrOp(_) => { true}
                _ => { false }
            }
        });
        if has_error.is_some() {
            return Err(anyhow::anyhow!(format!("表达式: {}有错误", input)));
        }
        Ok(Stack {
            stack,
        })
    }

    pub fn eval(&mut self) -> Option<f64> {
        let mut queue: Vec<f64> = Vec::<f64>::new();
        while self.stack.len() > 0 {
            let operation = self.stack.pop()?;
            match operation {
                StackItem::Number(x) => {
                    if self.stack.len() == 0 {
                        return Some(x);
                    } else {
                        queue.push(x);
                    }
                }
                StackItem::Operation(op) => {
                    while queue.len() > 1 {
                        let left = queue.pop()?;
                        let right = queue.pop()?;
                        match op {
                            Operation::Add => queue.push(left + right),
                            Operation::Subtract => queue.push(left - right),
                            Operation::Multiply => queue.push(left * right),
                            Operation::Divide => queue.push(left / right),
                        }
                    }
                    self.stack.push(StackItem::Number(queue.pop()?))
                }
                _ => {}
            };
        }
        return self.stack.pop()?.get_f64().ok();
    }
}

#[test]
fn addition() {
    let mut stack = Stack::init("+ 1 3");
    let result: f64 = stack.unwrap().eval().unwrap();
    assert_eq!(result, 4.0);
}

#[test]
fn subtraction() {
    let mut stack = Stack::init("- 6 3");
    let result: f64 = stack.unwrap().eval().unwrap();
    assert_eq!(result, 3.0);
}

#[test]
fn multiplication() {
    let mut stack = Stack::init("* 1 3");
    let result: f64 = stack.unwrap().eval().unwrap();
    assert_eq!(result, 3.0);
}

#[test]
fn division() {
    let mut stack = Stack::init("/ 6 2");
    let result: f64 = stack.unwrap().eval().unwrap();
    assert_eq!(result, 3.0);
}

#[test]
fn complex() {
    let mut stack = Stack::init("+ 3 * 4 2");
    let result: f64 = stack.unwrap().eval().unwrap();
    assert_eq!(result, 11.0);
}

#[test]
fn very_complex() {
    let mut stack = Stack::init("+ 3 * 4 2 3");
    let result: f64 = stack.unwrap().eval().unwrap();
    assert_eq!(result, 27.0);
}
