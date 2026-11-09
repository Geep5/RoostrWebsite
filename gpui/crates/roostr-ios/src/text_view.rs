//! The hidden view that takes the keyboard: `UIKeyInput` and `UITextInput` over the bridge's
//! [`Document`](crate::text_bridge::Document), so the keyboard types, composes, autocorrects
//! and predicts into the focused Rune field. It sits at the caret (so the candidate bar and
//! the system's text UI point there) and draws nothing; the Metal view under it gets the
//! touches.

use std::cell::RefCell;
use std::ops::Range;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{
    NSArray, NSAttributedStringKey, NSComparisonResult, NSCopying, NSDictionary, NSObjectProtocol,
    NSRange, NSSet, NSString,
};
use objc2_ui_kit::*;

use crate::host::{Shell, shell};
use crate::text_bridge::{Document, Input, checked_offset, checked_range, utf16_len};
use crate::views;

struct PositionIvars {
    offset: usize,
    session: u64,
}

define_class!(
    #[unsafe(super = UITextPosition)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrTextPosition"]
    #[ivars = PositionIvars]
    struct TextPosition;
    unsafe impl NSObjectProtocol for TextPosition {}
);

impl TextPosition {
    fn new(offset: usize, session: u64, mtm: MainThreadMarker) -> Retained<UITextPosition> {
        let this = Self::alloc(mtm).set_ivars(PositionIvars { offset, session });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.into_super()
    }
}

struct RangeIvars {
    range: Range<usize>,
    session: u64,
}

define_class!(
    #[unsafe(super = UITextRange)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrTextRange"]
    #[ivars = RangeIvars]
    struct TextRange;
    unsafe impl NSObjectProtocol for TextRange {}
    impl TextRange {
        #[unsafe(method_id(start))]
        fn start(&self) -> Retained<UITextPosition> {
            TextPosition::new(self.ivars().range.start, self.ivars().session, self.mtm())
        }
        #[unsafe(method_id(end))]
        fn end(&self) -> Retained<UITextPosition> {
            TextPosition::new(self.ivars().range.end, self.ivars().session, self.mtm())
        }
        #[unsafe(method(isEmpty))]
        fn is_empty(&self) -> bool {
            self.ivars().range.is_empty()
        }
    }
);

impl TextRange {
    fn new(range: Range<usize>, session: u64, mtm: MainThreadMarker) -> Retained<UITextRange> {
        let this = Self::alloc(mtm).set_ivars(RangeIvars { range, session });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.into_super()
    }
}

#[derive(Default)]
pub struct TextIvars {
    delegate: RefCell<Weak<ProtocolObject<dyn UITextInputDelegate>>>,
    marked_style: RefCell<Option<Retained<NSDictionary<NSAttributedStringKey, AnyObject>>>>,
}

define_class!(
    #[unsafe(super = UIView)]
    #[thread_kind = MainThreadOnly]
    #[name = "RoostrTextInputView"]
    #[ivars = TextIvars]
    pub struct TextInputView;

    unsafe impl NSObjectProtocol for TextInputView {}

    impl TextInputView {
        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            shell().is_some_and(|shell| shell.keyboard.focused())
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            let was = self.isFirstResponder();
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if resigned
                && was
                && let Some(shell) = shell()
                && !shell.keyboard.reconciling()
            {
                shell.keyboard.resigned(&shell);
            }
            resigned
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let composing = shell().is_some_and(|s| s.keyboard.doc.borrow().marked().is_some());
            let rest = views::presses_began(presses, true, composing);
            if !rest.is_empty() {
                unsafe { msg_send![super(self), pressesBegan: &*rest, withEvent: event] }
            }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let rest = views::presses_ended(presses);
            if !rest.is_empty() {
                unsafe { msg_send![super(self), pressesEnded: &*rest, withEvent: event] }
            }
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            let rest = views::presses_ended(presses);
            if !rest.is_empty() {
                unsafe { msg_send![super(self), pressesCancelled: &*rest, withEvent: event] }
            }
        }

        #[unsafe(method(canPerformAction:withSender:))]
        fn can_perform_action(&self, action: Sel, sender: Option<&AnyObject>) -> bool {
            match views::can_perform(action) {
                Some(can) => can,
                None => unsafe { msg_send![super(self), canPerformAction: action, withSender: sender] },
            }
        }

        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) {
            views::perform(objc2::sel!(cut:));
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            views::perform(objc2::sel!(copy:));
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            views::perform(objc2::sel!(paste:));
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            views::perform(objc2::sel!(selectAll:));
        }
    }

    unsafe impl UIKeyInput for TextInputView {
        #[unsafe(method(hasText))]
        fn has_text(&self) -> bool {
            read(|doc| !doc.is_empty()).unwrap_or(false)
        }

        #[unsafe(method(insertText:))]
        fn insert_text(&self, text: &NSString) {
            let text = text.to_string();
            edit(|doc| doc.insert_text(&text));
        }

        #[unsafe(method(deleteBackward))]
        fn delete_backward(&self) {
            edit(Document::delete_backward);
        }
    }

    unsafe impl UITextInputTraits for TextInputView {
        #[unsafe(method(keyboardType))]
        fn keyboard_type(&self) -> UIKeyboardType {
            UIKeyboardType::Default
        }

        #[unsafe(method(returnKeyType))]
        fn return_key_type(&self) -> UIReturnKeyType {
            UIReturnKeyType::Default
        }

        #[unsafe(method(isSecureTextEntry))]
        fn is_secure_text_entry(&self) -> bool {
            secure()
        }

        #[unsafe(method(autocorrectionType))]
        fn autocorrection_type(&self) -> UITextAutocorrectionType {
            if secure() { UITextAutocorrectionType::No } else { UITextAutocorrectionType::Yes }
        }

        #[unsafe(method(autocapitalizationType))]
        fn autocapitalization_type(&self) -> UITextAutocapitalizationType {
            if secure() {
                UITextAutocapitalizationType::None
            } else {
                UITextAutocapitalizationType::Sentences
            }
        }

        #[unsafe(method(spellCheckingType))]
        fn spell_checking_type(&self) -> UITextSpellCheckingType {
            if secure() { UITextSpellCheckingType::No } else { UITextSpellCheckingType::Yes }
        }

        #[unsafe(method(inlinePredictionType))]
        fn inline_prediction_type(&self) -> UITextInlinePredictionType {
            // An inline prediction draws its grey suggestion inside the text view, which draws
            // nothing: the suggestion bar above the keyboard offers it instead.
            UITextInlinePredictionType::No
        }

        #[unsafe(method(smartInsertDeleteType))]
        fn smart_insert_delete_type(&self) -> UITextSmartInsertDeleteType {
            // Smart insert edits around a paste or a deletion the bridge never saw.
            UITextSmartInsertDeleteType::No
        }
    }

    unsafe impl UITextInput for TextInputView {
        #[unsafe(method_id(textInRange:))]
        fn text_in_range(&self, range: &UITextRange) -> Option<Retained<NSString>> {
            (|| {
                let range = self.validated_range(range)?;
                read(|doc| doc.text_in(range)).flatten().map(|text| NSString::from_str(&text))
            })()
        }

        #[unsafe(method(replaceRange:withText:))]
        fn replace_range(&self, range: &UITextRange, text: &NSString) {
            if let Some(range) = self.validated_range(range) {
                let text = text.to_string();
                edit(|doc| doc.replace_range(range, &text));
            }
        }

        #[unsafe(method_id(selectedTextRange))]
        fn selected_text_range(&self) -> Option<Retained<UITextRange>> {
            read(|doc| doc.selection()).map(|range| self.new_range(range))
        }

        #[unsafe(method(setSelectedTextRange:))]
        fn set_selected_text_range(&self, range: Option<&UITextRange>) {
            if let Some(range) = range.and_then(|range| self.validated_range(range)) {
                edit(|doc| doc.set_selection(range));
            }
        }

        #[unsafe(method_id(markedTextRange))]
        fn marked_text_range(&self) -> Option<Retained<UITextRange>> {
            read(|doc| doc.marked()).flatten().map(|range| self.new_range(range))
        }

        #[unsafe(method_id(markedTextStyle))]
        fn marked_text_style(&self) -> Option<Retained<NSDictionary<NSAttributedStringKey, AnyObject>>> {
            self.ivars().marked_style.borrow().clone()
        }

        #[unsafe(method(setMarkedTextStyle:))]
        unsafe fn set_marked_text_style(&self, style: Option<&NSDictionary<NSAttributedStringKey, AnyObject>>) {
            *self.ivars().marked_style.borrow_mut() = style.map(|style| style.copy());
        }

        #[unsafe(method(setMarkedText:selectedRange:))]
        fn set_marked_text(&self, text: Option<&NSString>, selection: NSRange) {
            let text = text.map(|text| text.to_string()).unwrap_or_default();
            let selected = checked_range(selection.location, selection.length, utf16_len(&text))
                .unwrap_or_else(|| {
                    let end = utf16_len(&text);
                    end..end
                });
            edit(|doc| doc.set_marked_text(&text, selected));
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            edit(Document::unmark_text);
        }

        #[unsafe(method_id(beginningOfDocument))]
        fn beginning_of_document(&self) -> Retained<UITextPosition> {
            self.new_position(0)
        }

        #[unsafe(method_id(endOfDocument))]
        fn end_of_document(&self) -> Retained<UITextPosition> {
            self.new_position(read(Document::len).unwrap_or(0))
        }

        #[unsafe(method_id(textRangeFromPosition:toPosition:))]
        fn text_range(&self, start: &UITextPosition, end: &UITextPosition) -> Option<Retained<UITextRange>> {
            (|| {
                let start = self.validated_offset(start)?;
                let end = self.validated_offset(end)?;
                Some(self.new_range(start.min(end)..start.max(end)))
            })()
        }

        #[unsafe(method_id(positionFromPosition:offset:))]
        fn position_offset(&self, position: &UITextPosition, offset: isize) -> Option<Retained<UITextPosition>> {
            (|| {
                let position = self.validated_offset(position)?;
                let offset = checked_offset(position, offset, read(Document::len)?)?;
                Some(self.new_position(offset))
            })()
        }

        #[unsafe(method_id(positionFromPosition:inDirection:offset:))]
        fn position_direction(
            &self,
            position: &UITextPosition,
            direction: UITextLayoutDirection,
            offset: isize,
        ) -> Option<Retained<UITextPosition>> {
            (|| {
                let at = self.validated_offset(position)?;
                let len = read(Document::len)?;
                let offset = if direction == UITextLayoutDirection::Left {
                    offset.checked_neg()?
                } else if direction == UITextLayoutDirection::Right {
                    offset
                } else {
                    // One line: the document holds the text typed on the caret's line.
                    let edge = if direction == UITextLayoutDirection::Up { 0 } else { len };
                    return Some(self.new_position(edge));
                };
                Some(self.new_position(checked_offset(at, offset, len)?))
            })()
        }

        #[unsafe(method(comparePosition:toPosition:))]
        fn compare_position(&self, position: &UITextPosition, other: &UITextPosition) -> NSComparisonResult {
            let (Some(position), Some(other)) = (self.validated_offset(position), self.validated_offset(other)) else {
                return NSComparisonResult::Same;
            };
            match position.cmp(&other) {
                std::cmp::Ordering::Less => NSComparisonResult::Ascending,
                std::cmp::Ordering::Equal => NSComparisonResult::Same,
                std::cmp::Ordering::Greater => NSComparisonResult::Descending,
            }
        }

        #[unsafe(method(offsetFromPosition:toPosition:))]
        fn offset_from_position(&self, start: &UITextPosition, end: &UITextPosition) -> isize {
            let (Some(start), Some(end)) = (self.validated_offset(start), self.validated_offset(end)) else {
                return 0;
            };
            end as isize - start as isize
        }

        #[unsafe(method_id(inputDelegate))]
        fn input_delegate(&self) -> Option<Retained<ProtocolObject<dyn UITextInputDelegate>>> {
            self.ivars().delegate.borrow().load()
        }

        #[unsafe(method(setInputDelegate:))]
        fn set_input_delegate(&self, delegate: Option<&ProtocolObject<dyn UITextInputDelegate>>) {
            *self.ivars().delegate.borrow_mut() = delegate.map(Weak::new).unwrap_or_default();
        }

        #[unsafe(method_id(tokenizer))]
        fn input_tokenizer(&self) -> Retained<ProtocolObject<dyn UITextInputTokenizer>> {
            let tokenizer = unsafe {
                UITextInputStringTokenizer::initWithTextInput(UITextInputStringTokenizer::alloc(self.mtm()), self)
            };
            ProtocolObject::from_retained(tokenizer)
        }

        #[unsafe(method_id(positionWithinRange:farthestInDirection:))]
        fn farthest_position(&self, range: &UITextRange, direction: UITextLayoutDirection) -> Option<Retained<UITextPosition>> {
            (|| {
                let range = self.validated_range(range)?;
                let backwards = direction == UITextLayoutDirection::Left || direction == UITextLayoutDirection::Up;
                Some(self.new_position(if backwards { range.start } else { range.end }))
            })()
        }

        #[unsafe(method_id(characterRangeByExtendingPosition:inDirection:))]
        fn extending_range(&self, position: &UITextPosition, direction: UITextLayoutDirection) -> Option<Retained<UITextRange>> {
            (|| {
                let offset = self.validated_offset(position)?;
                let backwards = direction == UITextLayoutDirection::Left || direction == UITextLayoutDirection::Up;
                let at = if backwards { offset.checked_sub(1)? } else { offset };
                read(|doc| doc.character_at(at)).flatten().map(|range| self.new_range(range))
            })()
        }

        #[unsafe(method(baseWritingDirectionForPosition:inDirection:))]
        fn base_writing_direction(&self, _position: &UITextPosition, _direction: UITextStorageDirection) -> NSWritingDirection {
            NSWritingDirection::Natural
        }

        #[unsafe(method(setBaseWritingDirection:forRange:))]
        fn set_base_writing_direction(&self, _direction: NSWritingDirection, _range: &UITextRange) {
            // Rune lays a paragraph's direction out from its text.
        }

        #[unsafe(method(firstRectForRange:))]
        fn first_rect(&self, range: &UITextRange) -> CGRect {
            if self.validated_range(range).is_none() {
                return CGRect::ZERO;
            }
            caret_rect()
        }

        #[unsafe(method(caretRectForPosition:))]
        fn caret_rect(&self, position: &UITextPosition) -> CGRect {
            if self.validated_offset(position).is_none() {
                return CGRect::ZERO;
            }
            caret_rect()
        }

        #[unsafe(method_id(selectionRectsForRange:))]
        fn selection_rects(&self, _range: &UITextRange) -> Retained<NSArray<UITextSelectionRect>> {
            // Rune draws the selection itself and doesn't report its geometry.
            NSArray::new()
        }

        #[unsafe(method_id(closestPositionToPoint:))]
        fn closest_position(&self, _point: CGPoint) -> Option<Retained<UITextPosition>> {
            // Only the caret's place is known: the end of the selection.
            read(|doc| doc.selection().end).map(|offset| self.new_position(offset))
        }

        #[unsafe(method_id(closestPositionToPoint:withinRange:))]
        fn closest_position_within_range(&self, _point: CGPoint, range: &UITextRange) -> Option<Retained<UITextPosition>> {
            (|| {
                let range = self.validated_range(range)?;
                let caret = read(|doc| doc.selection().end)?;
                Some(self.new_position(caret.clamp(range.start, range.end)))
            })()
        }

        #[unsafe(method_id(characterRangeAtPoint:))]
        fn character_range_at_point(&self, _point: CGPoint) -> Option<Retained<UITextRange>> {
            None
        }

        #[unsafe(method_id(textInputView))]
        fn text_input_view(&self) -> Retained<UIView> {
            // Rects are in the Metal view's coordinates, where Rune reports the caret.
            self.superview().unwrap_or_else(|| objc2::Message::retain(self).into_super())
        }
    }
);

impl TextInputView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TextIvars::default());
        let frame = CGRect::new(CGPoint::ZERO, CGSize::new(1., 1.));
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setUserInteractionEnabled(false);
        this.setAlpha(0.01);
        this
    }

    /// A new session began (see [`Document::restart`]): UIKit reads the document again.
    pub fn document_changed(&self) {
        crate::keyboard::notify_document_changed(ProtocolObject::from_ref(self));
    }

    fn session(&self) -> Option<u64> {
        read(Document::session)
    }

    fn validated_offset(&self, position: &UITextPosition) -> Option<usize> {
        let position = position.downcast_ref::<TextPosition>()?;
        let len = read(Document::len)?;
        (Some(position.ivars().session) == self.session() && position.ivars().offset <= len)
            .then_some(position.ivars().offset)
    }

    fn validated_range(&self, range: &UITextRange) -> Option<Range<usize>> {
        let range = range.downcast_ref::<TextRange>()?;
        let ivars = range.ivars();
        (Some(ivars.session) == self.session() && read(|doc| doc.contains(&ivars.range))?)
            .then(|| ivars.range.clone())
    }

    fn new_position(&self, offset: usize) -> Retained<UITextPosition> {
        TextPosition::new(offset, self.session().unwrap_or(0), self.mtm())
    }

    fn new_range(&self, range: Range<usize>) -> Retained<UITextRange> {
        TextRange::new(range, self.session().unwrap_or(0), self.mtm())
    }
}

fn read<R>(f: impl FnOnce(&Document) -> R) -> Option<R> {
    let shell = shell()?;
    let doc = shell.keyboard.doc.try_borrow().ok()?;
    Some(f(&doc))
}

/// Changes the document as UIKit asked and sends Rune the input that mirrors it.
fn edit(f: impl FnOnce(&mut Document) -> Vec<Input>) {
    let Some(shell) = shell() else {
        return;
    };
    let out = match shell.keyboard.doc.try_borrow_mut() {
        Ok(mut doc) => f(&mut doc),
        Err(_) => return,
    };
    shell.keyboard.selected_all.set(false);
    shell.send(out);
}

fn secure() -> bool {
    read(Document::secure).unwrap_or(false)
}

/// The caret in the Metal view, as UIKit wants it (never zero-sized).
fn caret_rect() -> CGRect {
    let Some(caret) = shell().and_then(|shell: std::rc::Rc<Shell>| shell.keyboard.caret()) else {
        return CGRect::ZERO;
    };
    let [x, y, w, h] = crate::text_bridge::visible_caret(caret).map(f64::from);
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}
