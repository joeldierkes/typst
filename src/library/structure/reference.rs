use crate::library::prelude::*;

/// A reference to a label.
#[derive(Debug, Hash)]
pub struct RefNode(pub EcoString);

#[node(showable)]
impl RefNode {
    fn construct(_: &mut Machine, args: &mut Args) -> TypResult<Content> {
        Ok(Content::show(Self(args.expect("label")?)))
    }
}

impl Show for RefNode {
    fn unguard(&self, _: Selector) -> ShowNode {
        Self(self.0.clone()).pack()
    }

    fn encode(&self, _: StyleChain) -> Dict {
        dict! {
            "label" => Value::Str(self.0.clone().into()),
        }
    }

    fn realize(&self, _: &mut Context, _: StyleChain) -> TypResult<Content> {
        Ok(Content::Text(format_eco!("@{}", self.0)))
    }
}
