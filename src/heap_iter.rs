#[cfg(test)]
pub(crate) use crate::machine::gc::{IteratorUMP, StacklessPreOrderHeapIter};

use crate::atom_table::*;
use crate::machine::heap::*;
use crate::machine::stack::*;
use crate::types::*;

use modular_bitfield::prelude::*;

use std::ops::Deref;
use std::vec::Vec;

#[derive(BitfieldSpecifier, Clone, Copy, Debug, PartialEq, Eq)]
#[bits = 2]
enum IterStackLocTag {
    Iterable,
    Marked,
    PendingMark,
}

#[derive(BitfieldSpecifier, Clone, Copy, Debug, PartialEq, Eq)]
#[bits = 1]
pub enum HeapOrStackTag {
    Heap,
    Stack,
}

#[bitfield]
#[repr(u64)]
#[derive(Clone, Copy, Debug)]
pub struct IterStackLoc {
    pub value: B61,
    tag: IterStackLocTag,
    heap_or_stack: HeapOrStackTag,
}

impl IterStackLoc {
    #[inline]
    pub fn iterable_loc(h: usize, heap_or_stack: HeapOrStackTag) -> Self {
        IterStackLoc::new()
            .with_tag(IterStackLocTag::Iterable)
            .with_heap_or_stack(heap_or_stack)
            .with_value(h as u64)
    }

    #[inline]
    fn mark_loc(h: usize, heap_or_stack: HeapOrStackTag) -> Self {
        IterStackLoc::new()
            .with_tag(IterStackLocTag::Marked)
            .with_heap_or_stack(heap_or_stack)
            .with_value(h as u64)
    }

    #[inline]
    fn pending_mark_loc(h: usize, heap_or_stack: HeapOrStackTag) -> Self {
        IterStackLoc::new()
            .with_tag(IterStackLocTag::PendingMark)
            .with_heap_or_stack(heap_or_stack)
            .with_value(h as u64)
    }

    #[inline]
    pub fn is_marked(self) -> bool {
        self.tag() == IterStackLocTag::Marked
    }

    #[inline]
    pub fn is_pending_mark(self) -> bool {
        self.tag() == IterStackLocTag::PendingMark
    }

    #[inline]
    pub fn as_ref(self) -> Ref {
        match self.heap_or_stack() {
            HeapOrStackTag::Heap => {
                Ref::heap_cell(self.value() as usize)
            }
            HeapOrStackTag::Stack => {
                Ref::stack_cell(self.value() as usize)
            }
        }
    }
}

#[derive(Debug)]
pub struct StackfulPreOrderHeapIter<'a> {
    pub heap: &'a mut Vec<HeapCellValue>,
    pub machine_stack: &'a mut Stack,
    stack: Vec<IterStackLoc>,
    h: IterStackLoc,
}

impl<'a> Drop for StackfulPreOrderHeapIter<'a> {
    fn drop(&mut self) {
        while let Some(h) = self.stack.pop() {
            let cell = self.read_cell_mut(h);

            cell.set_forwarding_bit(false);
            cell.set_mark_bit(false);
        }

        self.heap.pop();
    }
}

pub trait FocusedHeapIter: Iterator<Item = HeapCellValue> {
    fn focus(&self) -> IterStackLoc;
}

impl<'a> FocusedHeapIter for StackfulPreOrderHeapIter<'a> {
    #[inline]
    fn focus(&self) -> IterStackLoc {
        self.h
    }
}

impl<'a> StackfulPreOrderHeapIter<'a> {
    #[inline]
    fn new(heap: &'a mut Vec<HeapCellValue>, stack: &'a mut Stack, cell: HeapCellValue) -> Self {
        let h = IterStackLoc::iterable_loc(heap.len(), HeapOrStackTag::Heap);
        heap.push(cell);

        Self {
            heap,
            h,
            machine_stack: stack,
            stack: vec![h],
        }
    }

    #[inline]
    fn forward_if_referent_marked(&mut self, loc: IterStackLoc) {
        read_heap_cell!(self.read_cell(loc),
            (HeapCellValueTag::Str |
             HeapCellValueTag::Lis |
             HeapCellValueTag::AttrVar |
             HeapCellValueTag::Var |
             HeapCellValueTag::PStrLoc, vh) => {
                if self.heap[vh].get_mark_bit() {
                    self.read_cell_mut(loc).set_forwarding_bit(true);
                }
            }
            (HeapCellValueTag::StackVar, vs) => {
                if self.machine_stack[vs].get_mark_bit() {
                    self.read_cell_mut(loc).set_forwarding_bit(true);
                }
            }
            _ => {}
        );
    }

    #[inline]
    pub fn push_stack(&mut self, h: IterStackLoc) {
        self.stack.push(h);
    }

    #[inline]
    pub fn read_cell_mut(&mut self, loc: IterStackLoc) -> &mut HeapCellValue {
        match loc.heap_or_stack() {
            HeapOrStackTag::Heap => {
                &mut self.heap[loc.value() as usize]
            }
            HeapOrStackTag::Stack => {
                &mut self.machine_stack[loc.value() as usize]
            }
        }
    }

    #[inline]
    pub fn read_cell(&self, loc: IterStackLoc) -> HeapCellValue {
        match loc.heap_or_stack() {
            HeapOrStackTag::Heap => {
                self.heap[loc.value() as usize]
            }
            HeapOrStackTag::Stack => {
                self.machine_stack[loc.value() as usize]
            }
        }
    }

    #[inline]
    pub fn stack_last(&self) -> Option<IterStackLoc> {
        for h in self.stack.iter().rev() {
            let is_readable_marked = h.is_marked();
            let cell = self.read_cell(*h);

            if cell.get_forwarding_bit() {
                return Some(*h);
            } else if cell.get_mark_bit() && !is_readable_marked {
                continue;
            }

            return Some(*h);
        }

        None
    }

    #[inline]
    pub fn pop_stack(&mut self) -> Option<HeapCellValue> {
        while let Some(h) = self.stack.pop() {
            let is_readable_marked = h.is_marked();

            self.h = h;
            let cell = self.read_cell_mut(h);

            if cell.get_forwarding_bit() {
                cell.set_forwarding_bit(false);
            } else if cell.get_mark_bit() && !is_readable_marked {
                cell.set_mark_bit(false);
                continue;
            }

            return Some(*cell);
        }

        None
    }

    #[inline]
    pub fn stack_len(&self) -> usize {
        self.stack.len()
    }

    fn push_if_unmarked(&mut self, loc: IterStackLoc) {
        let cell = self.read_cell_mut(loc);

        if !cell.get_mark_bit() {
            cell.set_mark_bit(true);
            self.stack.push(IterStackLoc::iterable_loc(loc.value() as usize, loc.heap_or_stack()));
        }
    }

    fn follow(&mut self) -> Option<HeapCellValue> {
        while let Some(h) = self.stack.pop() {
            if h.is_pending_mark() {
                self.push_if_unmarked(h);
                self.stack.push(IterStackLoc::mark_loc(h.value() as usize, h.heap_or_stack()));

                self.forward_if_referent_marked(h);
                continue;
            }

            self.h = h;

            let is_readable_marked = h.is_marked();
            let cell = self.read_cell_mut(h);

            if cell.get_forwarding_bit() {
                let copy = *cell;
                cell.set_forwarding_bit(false);
                return Some(copy);
            } else if cell.get_mark_bit() && !is_readable_marked {
                cell.set_mark_bit(false);
                continue;
            }

            read_heap_cell!(*cell,
               (HeapCellValueTag::Str | HeapCellValueTag::PStrLoc, vh) => {
                   let loc = IterStackLoc::iterable_loc(vh, HeapOrStackTag::Heap);

                   self.push_if_unmarked(loc);
                   self.stack.push(IterStackLoc::mark_loc(vh, HeapOrStackTag::Heap));
               }
               (HeapCellValueTag::Lis, vh) => {
                   let loc = IterStackLoc::iterable_loc(vh, HeapOrStackTag::Heap);

                   self.push_if_unmarked(loc);

                   self.stack.push(IterStackLoc::pending_mark_loc(vh + 1, HeapOrStackTag::Heap));
                   self.stack.push(IterStackLoc::mark_loc(vh, HeapOrStackTag::Heap));

                   self.forward_if_referent_marked(loc);

                   return Some(self.read_cell(h));
               }
               (HeapCellValueTag::AttrVar | HeapCellValueTag::Var, vh) => {
                   let loc = IterStackLoc::iterable_loc(vh, HeapOrStackTag::Heap);

                   self.push_if_unmarked(loc);
                   self.stack.push(IterStackLoc::mark_loc(vh, HeapOrStackTag::Heap));
                   self.forward_if_referent_marked(loc);
               }
               (HeapCellValueTag::StackVar, vs) => {
                   let loc = IterStackLoc::iterable_loc(vs, HeapOrStackTag::Stack);

                   self.push_if_unmarked(loc);
                   self.stack.push(IterStackLoc::mark_loc(vs, HeapOrStackTag::Stack));
                   self.forward_if_referent_marked(loc);
               }
               (HeapCellValueTag::PStrOffset, offset) => {
                   self.push_if_unmarked(IterStackLoc::iterable_loc(offset, HeapOrStackTag::Heap));
                   self.stack.push(IterStackLoc::iterable_loc((h.value()+1) as usize, HeapOrStackTag::Heap));

                   return Some(self.read_cell(h));
               }
               (HeapCellValueTag::PStr) => {
                   let tail_loc = IterStackLoc::iterable_loc((h.value()+1) as usize, HeapOrStackTag::Heap);

                   self.push_if_unmarked(IterStackLoc::iterable_loc(h.value() as usize, HeapOrStackTag::Heap));
                   self.stack.push(tail_loc);
                   self.forward_if_referent_marked(tail_loc);

                   return Some(self.read_cell(h));
               }
               (HeapCellValueTag::Atom, (_name, arity)) => {
                   let l = h.value() as usize;

                   for l in (l + 2 .. l + arity + 1).rev() {
                       self.stack.push(IterStackLoc::pending_mark_loc(l, HeapOrStackTag::Heap));
                   }

                   if arity > 0 {
                       let first_arg_loc = IterStackLoc::iterable_loc(l+1, HeapOrStackTag::Heap);

                       self.push_if_unmarked(first_arg_loc);
                       self.stack.push(IterStackLoc::mark_loc(l+1, HeapOrStackTag::Heap));
                       self.forward_if_referent_marked(first_arg_loc);
                   }

                   return Some(self.read_cell(h));
               }
               _ => {
                   return Some(*cell);
               }
            )
        }

        None
    }
}

impl<'a> Iterator for StackfulPreOrderHeapIter<'a> {
    type Item = HeapCellValue;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.follow()
    }
}

#[cfg(test)]
#[inline(always)]
pub(crate) fn stackless_preorder_iter(
    heap: &mut Vec<HeapCellValue>,
    cell: HeapCellValue,
) -> StacklessPreOrderHeapIter<IteratorUMP> {
    StacklessPreOrderHeapIter::<IteratorUMP>::new(heap, cell)
}

#[inline(always)]
pub(crate) fn stackful_preorder_iter<'a>(
    heap: &'a mut Vec<HeapCellValue>,
    stack: &'a mut Stack,
    cell: HeapCellValue,
) -> StackfulPreOrderHeapIter<'a> {
    StackfulPreOrderHeapIter::new(heap, stack, cell)
}

#[derive(Debug)]
pub(crate) struct PostOrderIterator<Iter: FocusedHeapIter> {
    focus: IterStackLoc,
    base_iter: Iter,
    base_iter_valid: bool,
    parent_stack: Vec<(usize, HeapCellValue, IterStackLoc)>, // number of children, parent node, focus.
}

impl<Iter: FocusedHeapIter> Deref for PostOrderIterator<Iter> {
    type Target = Iter;

    fn deref(&self) -> &Self::Target {
        &self.base_iter
    }
}

impl<Iter: FocusedHeapIter> PostOrderIterator<Iter> {
    pub(crate) fn new(base_iter: Iter) -> Self {
        PostOrderIterator {
            focus: IterStackLoc::iterable_loc(0, HeapOrStackTag::Heap),
            base_iter,
            base_iter_valid: true,
            parent_stack: vec![],
        }
    }
}

impl<Iter: FocusedHeapIter> Iterator for PostOrderIterator<Iter> {
    type Item = HeapCellValue;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((child_count, node, focus)) = self.parent_stack.pop() {
                if child_count == 0 {
                    self.focus = focus;
                    return Some(node);
                }

                self.parent_stack.push((child_count - 1, node, focus));
            }

            if self.base_iter_valid {
                if let Some(item) = self.base_iter.next() {
                    let focus = self.base_iter.focus();

                    read_heap_cell!(item,
                        (HeapCellValueTag::Atom, (_name, arity)) => {
                            self.parent_stack.push((arity, item, focus));
                        }
                        (HeapCellValueTag::Lis) => {
                            self.parent_stack.push((2, item, focus));
                        }
                        (HeapCellValueTag::PStr | HeapCellValueTag::PStrOffset) => {
                            self.parent_stack.push((1, item, focus));
                        }
                        _ => {
                            self.focus = focus;
                            return Some(item);
                        }
                    );

                    continue;
                } else {
                    self.base_iter_valid = false;
                }
            }

            if self.parent_stack.is_empty() {
                return None;
            }
        }
    }
}

impl<Iter: FocusedHeapIter> FocusedHeapIter for PostOrderIterator<Iter> {
    #[inline(always)]
    fn focus(&self) -> IterStackLoc {
        self.focus
    }
}

impl<Iter: FocusedHeapIter> PostOrderIterator<Iter> {
    /* return true if the term at heap offset idx_loc is a
     * direct/inlined subterm of a structure at the focus of
     * self.stack.last(). this function is used to determine, e.g.,
     * ownership of inlined code indices.
     */
    #[inline]
    pub(crate) fn direct_subterm_of_str(&self, idx_loc: usize) -> bool {
        if let Some((_child_count, item, focus)) = self.parent_stack.last() {
            read_heap_cell!(item,
                (HeapCellValueTag::Atom, (_name, arity)) => {
                    let focus = focus.value() as usize;
                    return focus + arity >= idx_loc && focus < idx_loc;
                }
                _ => {}
            );
        }

        false
    }
}

pub(crate) type LeftistPostOrderHeapIter<'a> = PostOrderIterator<StackfulPreOrderHeapIter<'a>>;

impl<'a> LeftistPostOrderHeapIter<'a> {
    #[inline]
    pub fn pop_stack(&mut self) {
        if let Some((child_count, ..)) = self.parent_stack.last() {
            for _ in 0 .. *child_count {
                self.base_iter.pop_stack();
            }

            self.parent_stack.pop();
        }
    }

    #[inline]
    pub fn parent_stack_len(&self) -> usize {
        self.parent_stack.len()
    }
}

#[inline]
pub(crate) fn stackful_post_order_iter<'a>(
    heap: &'a mut Heap,
    stack: &'a mut Stack,
    cell: HeapCellValue,
) -> LeftistPostOrderHeapIter<'a> {
    PostOrderIterator::new(StackfulPreOrderHeapIter::new(heap, stack, cell))
}

#[cfg(test)]
pub(crate) type RightistPostOrderHeapIter<'a> =
    PostOrderIterator<StacklessPreOrderHeapIter<'a, IteratorUMP>>;

#[cfg(test)]
#[inline]
pub(crate) fn stackless_post_order_iter<'a>(
    heap: &'a mut Heap,
    cell: HeapCellValue,
) -> RightistPostOrderHeapIter<'a> {
    PostOrderIterator::new(stackless_preorder_iter(heap, cell))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::mock_wam::*;


    #[test]
    fn heap_stackless_iter_tests() {
        let mut wam = MockWAM::new();

        let f_atom = atom!("f");
        let a_atom = atom!("a");
        let b_atom = atom!("b");

        wam.machine_st
           .heap
           .extend(functor!(f_atom, [atom(a_atom), atom(b_atom)]));

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, str_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 2)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom, 0)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom, 0)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.extend(functor!(
            f_atom,
            [
                atom(a_atom),
                atom(b_atom),
                atom(a_atom),
                cell(str_loc_as_cell!(0))
            ]
        ));

        for _ in 0..20 {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, str_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 4)
            );
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), str_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(str_loc_as_cell!(1));

        wam.machine_st.heap.extend(functor!(
            f_atom,
            [
                atom(a_atom),
                atom(b_atom),
                atom(a_atom),
                cell(str_loc_as_cell!(1))
            ]
        ));

        for _ in 0..200000 {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 4)
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), str_loc_as_cell!(1));
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        {
            wam.machine_st.heap.push(heap_loc_as_cell!(0));

            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                heap_loc_as_cell!(0),
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), heap_loc_as_cell!(0));
            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        // term  is: [a, b]
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(atom_as_cell!(a_atom));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(atom_as_cell!(b_atom));
        wam.machine_st.heap.push(empty_list_as_cell!());

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.pop();

        // now make the list cyclic.
        wam.machine_st.heap.push(heap_loc_as_cell!(0));

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        // first a 'dangling' partial string, later modified to be a two-part complete string,
        // then a three-part cyclic string involving an uncompacted list of chars.
        let pstr_var_cell = put_partial_string(&mut wam.machine_st.heap, "abc ", &mut wam.machine_st.atom_tbl);
        let pstr_cell = wam.machine_st.heap[pstr_var_cell.get_value() as usize];

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1),
            );

            assert!(iter.next().is_none());
        }

        assert_eq!(wam.machine_st.heap[0], pstr_cell);
        assert_eq!(wam.machine_st.heap[1], heap_loc_as_cell!(1));

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(2));

        let pstr_second_var_cell = put_partial_string(
            &mut wam.machine_st.heap,
            "def",
            &mut wam.machine_st.atom_tbl,
        );

        let pstr_second_cell = wam.machine_st.heap[pstr_second_var_cell.get_value() as usize];

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(3),
            );

            assert!(iter.next().is_none());
        }

        assert_eq!(wam.machine_st.heap[0], pstr_cell);
        assert_eq!(wam.machine_st.heap[1], pstr_loc_as_cell!(2));
        assert_eq!(wam.machine_st.heap[2], pstr_second_cell);
        assert_eq!(wam.machine_st.heap[3], heap_loc_as_cell!(3));

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(4));
        wam.machine_st.heap.push(pstr_offset_as_cell!(0));
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(2)));

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                pstr_loc_as_cell!(4),
            );

            let pstr_offset_cell = pstr_offset_as_cell!(0);

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);

            assert_eq!(iter.next(), None);
        }

        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[0]), pstr_cell);
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[1]), pstr_loc_as_cell!(2));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[2]), pstr_second_cell);
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[3]), pstr_loc_as_cell!(4));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[4]), pstr_offset_as_cell!(0));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[5]), fixnum_as_cell!(Fixnum::build_with(2)));

        wam.machine_st.heap.truncate(4);

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(wam.machine_st.heap.len() + 1));

        wam.machine_st.heap.push(pstr_offset_as_cell!(0));
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(0i64)));

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));
            let pstr_offset_cell = pstr_offset_as_cell!(0);

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_cell);

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(1i64)));

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));

            assert_eq!(iter.next(), None);

            assert_eq!(iter.heap[4], pstr_offset_as_cell!(0));
            assert_eq!(iter.heap[5], fixnum_as_cell!(Fixnum::build_with(1i64)));
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        let functor = functor!(f_atom, [atom(a_atom), atom(b_atom), atom(b_atom)]);

        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(empty_list_as_cell!());

        wam.machine_st.heap.extend(functor);

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            // drop the iterator before the iteration is complete to test
            // that modified heap cells are restored to their
            // pre-traversal state by the stackless iterator's Drop
            // instance.
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(wam.machine_st.heap[0], list_loc_as_cell!(1));
        assert_eq!(wam.machine_st.heap[1], str_loc_as_cell!(5));
        assert_eq!(wam.machine_st.heap[2], list_loc_as_cell!(3));
        assert_eq!(wam.machine_st.heap[3], str_loc_as_cell!(5));
        assert_eq!(wam.machine_st.heap[4], empty_list_as_cell!());

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            // drop the iterator before the iteration is complete to test
            // that modified heap cells are restored to their
            // pre-traversal state by the stackless iterator's Drop
            // instance.
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(wam.machine_st.heap[0], list_loc_as_cell!(1));
        assert_eq!(wam.machine_st.heap[1], str_loc_as_cell!(5));
        assert_eq!(wam.machine_st.heap[2], list_loc_as_cell!(3));
        assert_eq!(wam.machine_st.heap[3], str_loc_as_cell!(5));
        assert_eq!(wam.machine_st.heap[4], empty_list_as_cell!());

        wam.machine_st.heap[4] = list_loc_as_cell!(1);

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(heap_loc_as_cell!(1));
        wam.machine_st.heap.push(heap_loc_as_cell!(2));
        wam.machine_st.heap.push(heap_loc_as_cell!(3));
        wam.machine_st.heap.push(heap_loc_as_cell!(3));

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(iter.next().unwrap(), heap_loc_as_cell!(3));

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[0]), heap_loc_as_cell!(1));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[1]), heap_loc_as_cell!(2));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[2]), heap_loc_as_cell!(3));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[3]), heap_loc_as_cell!(3));

        wam.machine_st.heap.clear();

        // print L = [L|L].
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(list_loc_as_cell!(1));

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(iter.next().unwrap(), list_loc_as_cell!(1));
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next().unwrap(), list_loc_as_cell!(1));
            // this is what happens! this next line! We would like it not to happen though.
            assert_eq!(iter.next().unwrap(), list_loc_as_cell!(1));

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[0]), list_loc_as_cell!(1));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[1]), list_loc_as_cell!(1));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[2]), list_loc_as_cell!(1));

        wam.machine_st.heap.clear();

        // term is [X,f(Y),Z].
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(heap_loc_as_cell!(1));
        wam.machine_st.heap.push(heap_loc_as_cell!(3)); // 2
        wam.machine_st.heap.push(list_loc_as_cell!(4)); // 3
        wam.machine_st.heap.push(str_loc_as_cell!(6)); // 4
        wam.machine_st.heap.push(heap_loc_as_cell!(8));
        wam.machine_st.heap.push(atom_as_cell!(f_atom, 1)); // 6
        wam.machine_st.heap.push(heap_loc_as_cell!(11)); // 7
        wam.machine_st.heap.push(list_loc_as_cell!(9));
        wam.machine_st.heap.push(heap_loc_as_cell!(9));
        wam.machine_st.heap.push(empty_list_as_cell!());

        wam.machine_st.heap.push(attr_var_as_cell!(11)); // linked from 7.
        wam.machine_st.heap.push(heap_loc_as_cell!(12));

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(iter.next().unwrap(), list_loc_as_cell!(1));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(4)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(9)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(9)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                attr_var_as_cell!(11)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1)
            );
            assert_eq!(iter.next(), None);
        }

        // now populate the attributes list. the iteration must not change.
        let clpz_atom = atom!("clpz");
        let p_atom = atom!("p");

        wam.machine_st.heap.pop();

        wam.machine_st.heap.push(heap_loc_as_cell!(13)); // 12
        wam.machine_st.heap.push(list_loc_as_cell!(14)); // 13
        wam.machine_st.heap.push(str_loc_as_cell!(16)); // 14
        wam.machine_st.heap.push(heap_loc_as_cell!(19)); // 15
        wam.machine_st.heap.push(atom_as_cell!(clpz_atom, 2)); // 16
        wam.machine_st.heap.push(atom_as_cell!(a_atom)); // 17
        wam.machine_st.heap.push(atom_as_cell!(b_atom)); // 18
        wam.machine_st.heap.push(list_loc_as_cell!(20)); // 19
        wam.machine_st.heap.push(str_loc_as_cell!(22)); // 20
        wam.machine_st.heap.push(empty_list_as_cell!()); // 21
        wam.machine_st.heap.push(atom_as_cell!(p_atom, 1)); // 22
        wam.machine_st.heap.push(heap_loc_as_cell!(23)); // 23

        {
            let mut iter = stackless_preorder_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(iter.next().unwrap(), list_loc_as_cell!(1));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(4)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(9)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(9)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                attr_var_as_cell!(11)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1)
            );
            assert_eq!(iter.next(), None);
        }

        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[0]), list_loc_as_cell!(1));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[1]), heap_loc_as_cell!(1));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[2]), heap_loc_as_cell!(3));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[3]), list_loc_as_cell!(4));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[4]), str_loc_as_cell!(6));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[5]), heap_loc_as_cell!(8));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[6]), atom_as_cell!(f_atom, 1));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[7]), heap_loc_as_cell!(11));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[8]), list_loc_as_cell!(9));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[9]), heap_loc_as_cell!(9));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[10]), empty_list_as_cell!());
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[11]), attr_var_as_cell!(11));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[12]), heap_loc_as_cell!(13));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[13]), list_loc_as_cell!(14));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[14]), str_loc_as_cell!(16));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[15]), heap_loc_as_cell!(19));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[16]), atom_as_cell!(clpz_atom, 2));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[17]), atom_as_cell!(a_atom));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[18]), atom_as_cell!(b_atom));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[19]), list_loc_as_cell!(20));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[20]), str_loc_as_cell!(22));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[21]), empty_list_as_cell!());
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[22]), atom_as_cell!(p_atom, 1));
        assert_eq!(unmark_cell_bits!(wam.machine_st.heap[23]), heap_loc_as_cell!(23));

        wam.machine_st.heap.clear();

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                fixnum_as_cell!(Fixnum::build_with(0))
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                fixnum_as_cell!(Fixnum::build_with(0))
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(wam.machine_st.heap.len(), 0);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(str_loc_as_cell!(1));

        wam.machine_st.heap.push(atom_as_cell!(atom!("g"),2));
        wam.machine_st.heap.push(heap_loc_as_cell!(0));
        wam.machine_st.heap.push(atom_as_cell!(atom!("y")));

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                str_loc_as_cell!(1),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("g"),2)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("y"))
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                str_loc_as_cell!(1)
            );

            assert!(iter.next().is_none());
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(atom_as_cell!(atom!("g"),2));
        wam.machine_st.heap.push(str_loc_as_cell!(0));
        wam.machine_st.heap.push(atom_as_cell!(atom!("y")));

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                str_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("g"),2)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("y"))
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                str_loc_as_cell!(0)
            );

            assert!(iter.next().is_none());
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(str_loc_as_cell!(1));
        wam.machine_st.heap.push(atom_as_cell!(atom!("g"), 2));
        wam.machine_st.heap.push(heap_loc_as_cell!(0));
        wam.machine_st.heap.push(atom_as_cell!(atom!("y")));
        wam.machine_st.heap.push(atom_as_cell!(atom!("="), 2));
        wam.machine_st.heap.push(atom_as_cell!(atom!("X")));
        wam.machine_st.heap.push(heap_loc_as_cell!(0));
        wam.machine_st.heap.push(list_loc_as_cell!(8));
        wam.machine_st.heap.push(str_loc_as_cell!(4));
        wam.machine_st.heap.push(empty_list_as_cell!());

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                heap_loc_as_cell!(7),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(8)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("="), 2)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("g"), 2)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("y"))
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("X"))
            );

            assert!(iter.next().is_none());
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(wam.machine_st.heap[0], str_loc_as_cell!(1));
        assert_eq!(wam.machine_st.heap[1], atom_as_cell!(atom!("g"), 2));
        assert_eq!(wam.machine_st.heap[2], heap_loc_as_cell!(0));
        assert_eq!(wam.machine_st.heap[3], atom_as_cell!(atom!("y")));
        assert_eq!(wam.machine_st.heap[4], atom_as_cell!(atom!("="), 2));
        assert_eq!(wam.machine_st.heap[5], atom_as_cell!(atom!("X")));
        assert_eq!(wam.machine_st.heap[6], heap_loc_as_cell!(0));
        assert_eq!(wam.machine_st.heap[7], list_loc_as_cell!(8));
        assert_eq!(wam.machine_st.heap[8], str_loc_as_cell!(4));
        assert_eq!(wam.machine_st.heap[9], empty_list_as_cell!());

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(atom_as_cell!(atom!("f"), 2));
        wam.machine_st.heap.push(heap_loc_as_cell!(1));
        wam.machine_st.heap.push(heap_loc_as_cell!(1));

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                str_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("f"), 2)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1)
            );

            assert!(iter.next().is_none());
        }

        assert_eq!(wam.machine_st.heap[0], atom_as_cell!(atom!("f"), 2));
        assert_eq!(wam.machine_st.heap[1], heap_loc_as_cell!(1));
        assert_eq!(wam.machine_st.heap[2], heap_loc_as_cell!(1));

        wam.machine_st.heap.clear();

        // representation of one of the heap terms as in issue #1384.
/*
        wam.machine_st.heap.push(list_loc_as_cell!(7));
        wam.machine_st.heap.push(heap_loc_as_cell!(0));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(list_loc_as_cell!(5));
        wam.machine_st.heap.push(empty_list_as_cell!());
        wam.machine_st.heap.push(heap_loc_as_cell!(2));
        wam.machine_st.heap.push(heap_loc_as_cell!(2));
        wam.machine_st.heap.push(empty_list_as_cell!());
        wam.machine_st.heap.push(heap_loc_as_cell!(3));

        {
            let mut iter = stackless_preorder_iter(
                &mut wam.machine_st.heap,
                heap_loc_as_cell!(0),
            );

            while let Some(_) = iter.next() {
                print_heap_terms(iter.heap.iter(), 0);
                println!("");
            }

            /*
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("f"), 2)
            );

            assert!(iter.next().is_none());
            */
        }
*/
    }

    #[test]
    fn heap_stackful_iter_tests() {
        let mut wam = MockWAM::new();

        let f_atom = atom!("f");
        let a_atom = atom!("a");
        let b_atom = atom!("b");

        wam.machine_st.heap
            .extend(functor!(f_atom, [atom(a_atom), atom(b_atom)]));

        {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                str_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 2)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        wam.machine_st.heap.extend(functor!(
            f_atom,
            [
                atom(a_atom),
                atom(b_atom),
                atom(a_atom),
                cell(str_loc_as_cell!(0))
            ]
        ));

        for _ in 0..20 {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                str_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 4)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), str_loc_as_cell!(0));
            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        {
            wam.machine_st.heap.push(heap_loc_as_cell!(0));

            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            let mut var = heap_loc_as_cell!(0);

            // self-referencing variables are copied with their forwarding
            // and marking bits set to true. it suffices to check only the
            // forwarding bit to detect cycles of all kinds, including
            // unbound/self-referencing variables.

            var.set_forwarding_bit(true);
            var.set_mark_bit(true);

            assert_eq!(iter.next().unwrap(), var);
            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        {
            // mutually referencing variables.
            wam.machine_st.heap.push(heap_loc_as_cell!(1));
            wam.machine_st.heap.push(heap_loc_as_cell!(0));

            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        // term  is: [a, b]
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(atom_as_cell!(a_atom));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(atom_as_cell!(b_atom));
        wam.machine_st.heap.push(empty_list_as_cell!());

        {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();

        // now make the list cyclic.
        wam.machine_st.heap.push(heap_loc_as_cell!(0));

        {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            // the cycle will be iterated twice before being detected.
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );

            assert_eq!(iter.next(), None);
        }

        {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            // cut the iteration short to check that all cells are
            // unmarked and unforwarded by the Drop instance of
            // StackfulPreOrderHeapIter.

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(wam.machine_st.heap[0], list_loc_as_cell!(1));
        assert_eq!(wam.machine_st.heap[1], atom_as_cell!(a_atom));
        assert_eq!(wam.machine_st.heap[2], list_loc_as_cell!(3));
        assert_eq!(wam.machine_st.heap[3], atom_as_cell!(b_atom));
        assert_eq!(wam.machine_st.heap[4], heap_loc_as_cell!(0));

        wam.machine_st.heap.clear();

        // first a 'dangling' partial string, later modified to be a
        // two-part complete string, then a three-part cyclic string
        // involving an uncompacted list of chars.

        let pstr_var_cell = put_partial_string(&mut wam.machine_st.heap, "abc ", &mut wam.machine_st.atom_tbl);
        let pstr_cell = wam.machine_st.heap[pstr_var_cell.get_value() as usize];

        {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1),
            );

            assert_eq!(iter.next(), None);
        }

        // here

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(heap_loc_as_cell!(2));

        let pstr_second_var_cell = put_partial_string(&mut wam.machine_st.heap, "def", &mut wam.machine_st.atom_tbl);
        let pstr_second_cell = wam.machine_st.heap[pstr_second_var_cell.get_value() as usize];

        {
            let mut iter = stackful_preorder_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(3),
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(wam.machine_st.heap.len() + 1));

        wam.machine_st.heap.push(pstr_offset_as_cell!(0));
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(0i64)));

        {
            let mut iter = stackful_preorder_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                pstr_loc_as_cell!(0),
            );

            let pstr_offset_cell = pstr_offset_as_cell!(0);

            // pstr_offset_cell.set_forwarding_bit(true);

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_cell);
            assert_eq!(iter.next().unwrap(), fixnum_as_cell!(Fixnum::build_with(0i64)));

            assert_eq!(iter.next(), None);
        }

        /*
        {
            let mut iter = HeapPStrIter::new(&wam.machine_st.heap, 0);
            let string: String = iter.chars().collect();
            assert_eq!(string, "abc def");
        }
        */

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(1i64)));

        {
            let mut iter = stackful_preorder_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                pstr_loc_as_cell!(0),
            );

            let pstr_offset_cell = pstr_offset_as_cell!(0);

            // pstr_offset_cell.set_forwarding_bit(true);

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_cell);
            assert_eq!(iter.next().unwrap(), fixnum_as_cell!(Fixnum::build_with(1i64)));

            let h = iter.focus();

            assert_eq!(h.value(), 5);
            assert_eq!(unmark_cell_bits!(iter.heap[4]), pstr_offset_as_cell!(0));
            assert_eq!(unmark_cell_bits!(iter.heap[5]), fixnum_as_cell!(Fixnum::build_with(1i64)));

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        let functor = functor!(f_atom, [atom(a_atom), atom(b_atom), atom(b_atom)]);

        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(empty_list_as_cell!());

        wam.machine_st.heap.extend(functor);

        {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap[4] = list_loc_as_cell!(1);

        {
            let mut iter = stackful_preorder_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            let mut link_back = list_loc_as_cell!(1);

            link_back.set_forwarding_bit(true);
            link_back.set_mark_bit(true);

            assert_eq!(iter.next().unwrap(), link_back);

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(list_loc_as_cell!(1));

        {
            let mut iter = StackfulPreOrderHeapIter::new(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            let mut cyclic_link = list_loc_as_cell!(1);

            cyclic_link.set_forwarding_bit(true);
            cyclic_link.set_mark_bit(true);

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );
            assert_eq!(iter.next().unwrap(), cyclic_link);

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(pstr_as_cell!(atom!("a string")));
        wam.machine_st.heap.push(empty_list_as_cell!());

        {
            let mut iter = stackful_preorder_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                pstr_as_cell!(atom!("a string"))
            );

            assert_eq!(
                iter.next().unwrap(),
                empty_list_as_cell!()
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.clear();

        wam.machine_st.heap.push(str_loc_as_cell!(1));
        wam.machine_st.heap.push(atom_as_cell!(atom!("g"), 2));
        wam.machine_st.heap.push(heap_loc_as_cell!(0));
        wam.machine_st.heap.push(atom_as_cell!(atom!("y")));
        wam.machine_st.heap.push(atom_as_cell!(atom!("="), 2));
        wam.machine_st.heap.push(atom_as_cell!(atom!("X")));
        wam.machine_st.heap.push(heap_loc_as_cell!(0));
        wam.machine_st.heap.push(list_loc_as_cell!(8));
        wam.machine_st.heap.push(str_loc_as_cell!(4));
        wam.machine_st.heap.push(empty_list_as_cell!());

        {
            let mut iter = stackful_preorder_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("g"), 2)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(atom!("y"))
            );

            assert!(iter.next().is_none());
        }
    }

    #[test]
    fn heap_stackful_post_order_iter() {
        let mut wam = MockWAM::new();

        let f_atom = atom!("f");
        let a_atom = atom!("a");
        let b_atom = atom!("b");

        wam.machine_st.heap
            .extend(functor!(f_atom, [atom(a_atom), atom(b_atom)]));

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                str_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 2)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        wam.machine_st.heap.extend(functor!(
            f_atom,
            [
                atom(a_atom),
                atom(b_atom),
                atom(a_atom),
                cell(str_loc_as_cell!(0))
            ]
        ));

        for _ in 0..20 { // 0000 {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                str_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), str_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 4)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        {
            wam.machine_st.heap.push(heap_loc_as_cell!(0));

            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            let mut var = heap_loc_as_cell!(0);

            // self-referencing variables are copied with their forwarding
            // and marking bits set to true. it suffices to check only the
            // forwarding bit to detect cycles of all kinds, including
            // unbound/self-referencing variables.

            var.set_forwarding_bit(true);
            var.set_mark_bit(true);

            assert_eq!(iter.next().unwrap(), var);
            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        {
            // mutually referencing variables.
            wam.machine_st.heap.push(heap_loc_as_cell!(1));
            wam.machine_st.heap.push(heap_loc_as_cell!(0));

            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        // term  is: [a, b]
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(atom_as_cell!(a_atom));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(atom_as_cell!(b_atom));
        wam.machine_st.heap.push(empty_list_as_cell!());

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();

        // now make the list cyclic.
        wam.machine_st.heap.push(heap_loc_as_cell!(0));

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            // the cycle will be iterated twice before being detected.
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            // cut the iteration short to check that all cells are
            // unmarked and unforwarded by the Drop instance of
            // StackfulPreOrderHeapIter.

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(wam.machine_st.heap[0], list_loc_as_cell!(1));
        assert_eq!(wam.machine_st.heap[1], atom_as_cell!(a_atom));
        assert_eq!(wam.machine_st.heap[2], list_loc_as_cell!(3));
        assert_eq!(wam.machine_st.heap[3], atom_as_cell!(b_atom));
        assert_eq!(wam.machine_st.heap[4], heap_loc_as_cell!(0));

        wam.machine_st.heap.clear();

        // first a 'dangling' partial string, later modified to be a
        // two-part complete string, then a three-part cyclic string
        // involving an uncompacted list of chars.

        let pstr_var_cell = put_partial_string(&mut wam.machine_st.heap, "abc ", &mut wam.machine_st.atom_tbl);
        let pstr_cell = wam.machine_st.heap[pstr_var_cell.get_value() as usize];

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                pstr_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1),
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(2));

        let pstr_second_var_cell = put_partial_string(&mut wam.machine_st.heap, "def", &mut wam.machine_st.atom_tbl);
        let pstr_second_cell = wam.machine_st.heap[pstr_second_var_cell.get_value() as usize];

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                pstr_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(3),
            );
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(wam.machine_st.heap.len() + 1));

        wam.machine_st.heap.push(pstr_offset_as_cell!(0));
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(0i64)));

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                pstr_loc_as_cell!(0),
            );

            assert_eq!(iter.next().unwrap(), fixnum_as_cell!(Fixnum::build_with(0i64)));
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(1i64)));

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                pstr_loc_as_cell!(0),
            );

            assert_eq!(iter.next().unwrap(), fixnum_as_cell!(Fixnum::build_with(1i64)));
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        let functor = functor!(f_atom, [atom(a_atom), atom(b_atom), atom(b_atom)]);

        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(empty_list_as_cell!());

        wam.machine_st.heap.extend(functor);

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap[4] = list_loc_as_cell!(1);

        {
            let mut iter = stackful_post_order_iter(
                &mut wam.machine_st.heap,
                &mut wam.machine_st.stack,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            let mut link_back = list_loc_as_cell!(1);

            link_back.set_forwarding_bit(true);
            link_back.set_mark_bit(true);

            assert_eq!(iter.next().unwrap(), link_back);

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);
        wam.machine_st.heap.clear();
    }

    #[test]
    fn heap_stackless_post_order_iter() {
        let mut wam = MockWAM::new();

        let f_atom = atom!("f");
        let a_atom = atom!("a");
        let b_atom = atom!("b");

        wam.machine_st.heap.extend(functor!(f_atom, [atom(a_atom), atom(b_atom)]));

        {
            let mut iter = stackless_post_order_iter(
                &mut wam.machine_st.heap,
                str_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 2)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        wam.machine_st.heap.extend(functor!(
            f_atom,
            [
                atom(a_atom),
                atom(b_atom),
                atom(a_atom),
                cell(str_loc_as_cell!(0))
            ]
        ));

        for _ in 0..20 {
            let mut iter = stackless_post_order_iter(
                &mut wam.machine_st.heap,
                str_loc_as_cell!(0),
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), str_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 4)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        {
            wam.machine_st.heap.push(heap_loc_as_cell!(0));

            let mut iter = stackless_post_order_iter(
                &mut wam.machine_st.heap,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );
            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        {
            // mutually referencing variables.
            wam.machine_st.heap.push(heap_loc_as_cell!(1));
            wam.machine_st.heap.push(heap_loc_as_cell!(0));

            let mut iter = stackless_post_order_iter(
                &mut wam.machine_st.heap,
                heap_loc_as_cell!(0),
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        // term  is: [a, b]
        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(atom_as_cell!(a_atom));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(atom_as_cell!(b_atom));
        wam.machine_st.heap.push(empty_list_as_cell!());

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();

        // now make the list cyclic.
        wam.machine_st.heap.push(heap_loc_as_cell!(0));

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            // the cycle will be iterated twice before being detected.
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            // cut the iteration short to check that all cells are
            // unmarked and unforwarded by the Drop instance of
            // StacklessPreOrderHeapIter.

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(0)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
        }

        all_cells_unmarked(&wam.machine_st.heap);

        assert_eq!(wam.machine_st.heap[0], list_loc_as_cell!(1));
        assert_eq!(wam.machine_st.heap[1], atom_as_cell!(a_atom));
        assert_eq!(wam.machine_st.heap[2], list_loc_as_cell!(3));
        assert_eq!(wam.machine_st.heap[3], atom_as_cell!(b_atom));
        assert_eq!(wam.machine_st.heap[4], heap_loc_as_cell!(0));

        wam.machine_st.heap.clear();

        // first a 'dangling' partial string, later modified to be a
        // two-part complete string, then a three-part cyclic string
        // involving an uncompacted list of chars.

        let pstr_var_cell = put_partial_string(&mut wam.machine_st.heap, "abc ", &mut wam.machine_st.atom_tbl);
        let pstr_cell = wam.machine_st.heap[pstr_var_cell.get_value() as usize];

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(1),
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(2));

        let pstr_second_var_cell = put_partial_string(
            &mut wam.machine_st.heap,
            "def",
            &mut wam.machine_st.atom_tbl,
        );

        let pstr_second_cell = wam.machine_st.heap[pstr_second_var_cell.get_value() as usize];

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                heap_loc_as_cell!(3),
            );

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(pstr_loc_as_cell!(wam.machine_st.heap.len() + 1));

        wam.machine_st.heap.push(pstr_offset_as_cell!(0));
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(0)));

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));
            let mut pstr_loc_cell = pstr_loc_as_cell!(0);

            pstr_loc_cell.set_forwarding_bit(true);

            // assert_eq!(iter.next().unwrap(), fixnum_as_cell!(Fixnum::build_with(0i64)));
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap.pop();
        wam.machine_st.heap.push(fixnum_as_cell!(Fixnum::build_with(1)));

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, pstr_loc_as_cell!(0));

            //assert_eq!(iter.next().unwrap(), fixnum_as_cell!(Fixnum::build_with(1)));
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_offset_as_cell!(0));

            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_second_cell);
            assert_eq!(unmark_cell_bits!(iter.next().unwrap()), pstr_cell);

            assert_eq!(iter.next(), None);
        }

        wam.machine_st.heap.clear();

        let functor = functor!(f_atom, [atom(a_atom), atom(b_atom), atom(b_atom)]);

        wam.machine_st.heap.push(list_loc_as_cell!(1));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(list_loc_as_cell!(3));
        wam.machine_st.heap.push(str_loc_as_cell!(5));
        wam.machine_st.heap.push(empty_list_as_cell!());

        wam.machine_st.heap.extend(functor);

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                empty_list_as_cell!()
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);

        wam.machine_st.heap[4] = list_loc_as_cell!(1);

        {
            let mut iter = stackless_post_order_iter(&mut wam.machine_st.heap, heap_loc_as_cell!(0));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(b_atom)
            );
            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(a_atom)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                atom_as_cell!(f_atom, 3)
            );

            assert_eq!(iter.next().unwrap(), list_loc_as_cell!(1));

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(3)
            );

            assert_eq!(
                unmark_cell_bits!(iter.next().unwrap()),
                list_loc_as_cell!(1)
            );

            assert_eq!(iter.next(), None);
        }

        all_cells_unmarked(&wam.machine_st.heap);
    }
}
