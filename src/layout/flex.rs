use super::*;

/// Layouts boxes flex-like.
///
/// The boxes are arranged in "lines", each line having the height of its
/// biggest box. When a box does not fit on a line anymore horizontally,
/// a new line is started.
///
/// The flex layouter does not actually compute anything until the `finish`
/// method is called. The reason for this is the flex layouter will have
/// the capability to justify its layouts, later. To find a good justification
/// it needs total information about the contents.
///
/// There are two different kinds units that can be added to a flex run:
/// Normal layouts and _glue_. _Glue_ layouts are only written if a normal
/// layout follows and a glue layout is omitted if the following layout
/// flows into a new line. A _glue_ layout is typically used for a space character
/// since it prevents a space from appearing in the beginning or end of a line.
/// However, it can be any layout.
pub struct FlexLayouter {
    ctx: FlexContext,
    units: Vec<FlexUnit>,

    stack: StackLayouter,
    usable_width: Size,
    run: FlexRun,
    cached_glue: Option<Layout>,
}

/// The context for flex layouting.
#[derive(Debug, Copy, Clone)]
pub struct FlexContext {
    pub space: LayoutSpace,
    /// The spacing between two lines of boxes.
    pub flex_spacing: Size,
    pub extra_space: Option<LayoutSpace>,
}

enum FlexUnit {
    /// A content unit to be arranged flexibly.
    Boxed(Layout),
    /// A unit which acts as glue between two [`FlexUnit::Boxed`] units and
    /// is only present if there was no flow break in between the two
    /// surrounding boxes.
    Glue(Layout),
}

struct FlexRun {
    content: Vec<(Size, Layout)>,
    size: Size2D,
}

impl FlexLayouter {
    /// Create a new flex layouter.
    pub fn new(ctx: FlexContext) -> FlexLayouter {
        FlexLayouter {
            ctx,
            units: vec![],

            stack: StackLayouter::new(StackContext {
                space: ctx.space,
                extra_space: ctx.extra_space,
            }),

            usable_width: ctx.space.usable().x,
            run: FlexRun {
                content: vec![],
                size: Size2D::zero()
            },
            cached_glue: None,
        }
    }

    /// This layouter's context.
    pub fn ctx(&self) -> FlexContext {
        self.ctx
    }

    /// Add a sublayout.
    pub fn add(&mut self, layout: Layout) {
        self.units.push(FlexUnit::Boxed(layout));
    }

    /// Add a glue layout which can be replaced by a line break.
    pub fn add_glue(&mut self, glue: Layout) {
        self.units.push(FlexUnit::Glue(glue));
    }

    /// Compute the justified layout.
    ///
    /// The layouter is not consumed by this to prevent ownership problems
    /// with borrowed layouters. The state of the layouter is not reset.
    /// Therefore, it should not be further used after calling `finish`.
    pub fn finish(&mut self) -> LayoutResult<MultiLayout> {
        // Move the units out of the layout because otherwise, we run into
        // ownership problems.
        let units = std::mem::replace(&mut self.units, vec![]);
        for unit in units {
            match unit {
                FlexUnit::Boxed(boxed) => self.layout_box(boxed)?,
                FlexUnit::Glue(glue) => self.layout_glue(glue),
            }
        }

        // Finish the last flex run.
        self.finish_run()?;

        self.stack.finish()
    }

    /// Layout a content box into the current flex run or start a new run if
    /// it does not fit.
    fn layout_box(&mut self, boxed: Layout) -> LayoutResult<()> {
        let glue_width = self
            .cached_glue
            .as_ref()
            .map(|layout| layout.dimensions.x)
            .unwrap_or(Size::zero());

        let new_line_width = self.run.size.x + glue_width + boxed.dimensions.x;

        if self.overflows_line(new_line_width) {
            self.cached_glue = None;

            // If the box does not even fit on its own line, then we try
            // it in the next space, or we have to give up if there is none.
            if self.overflows_line(boxed.dimensions.x) {
                if self.ctx.extra_space.is_some() {
                    self.stack.finish_layout(true)?;
                    return self.layout_box(boxed);
                } else {
                    return Err(LayoutError::NotEnoughSpace("cannot fit box into flex run"));
                }
            }

            self.finish_run()?;
        } else {
            // Only add the glue if we did not move to a new line.
            self.flush_glue();
        }

        self.add_to_run(boxed);

        Ok(())
    }

    fn layout_glue(&mut self, glue: Layout) {
        self.flush_glue();
        self.cached_glue = Some(glue);
    }

    fn flush_glue(&mut self) {
        if let Some(glue) = self.cached_glue.take() {
            let new_line_width = self.run.size.x + glue.dimensions.x;
            if !self.overflows_line(new_line_width) {
                self.add_to_run(glue);
            }
        }
    }

    fn add_to_run(&mut self, layout: Layout) {
        let x = self.run.size.x;

        self.run.size.x += layout.dimensions.x;
        self.run.size.y = crate::size::max(self.run.size.y, layout.dimensions.y);

        self.run.content.push((x, layout));
    }

    fn finish_run(&mut self) -> LayoutResult<()> {
        self.run.size.y += self.ctx.flex_spacing;

        let mut actions = LayoutActionList::new();
        for (x, layout) in self.run.content.drain(..) {
            let position = Size2D::with_x(x);
            actions.add_layout(position, layout);
        }

        self.stack.add(Layout {
            dimensions: self.run.size,
            actions: actions.into_vec(),
            debug_render: false,
        })?;

        self.run.size = Size2D::zero();

        Ok(())
    }

    /// Whether this layouter contains any items.
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    fn overflows_line(&self, line: Size) -> bool {
        line > self.usable_width
    }
}
