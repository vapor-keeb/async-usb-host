use core::mem::MaybeUninit;
use core::{array, ptr};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use embassy_futures::select::Either;

// Forward declaration of SlotState if needed, or ensure it's defined before use.
// Assuming SlotState is defined later in the file as shown in the context.

/// Future for selecting between two potentially `!Unpin` futures.
///
/// Similar to `embassy_futures::select::select`, but owns the futures
/// and manages their state internally, allowing for `!Unpin` types.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct SelectPin2<Fut1: Future, Fut2: Future> {
    fut1: MaybeUninit<Fut1>,
    fut2: MaybeUninit<Fut2>,
    /// 0: fut1 state, 1: fut2 state
    states: [SlotState; 2],
}

impl<Fut1: Future, Fut2: Future> SelectPin2<Fut1, Fut2> {
    /// Creates a new, empty selector.
    ///
    /// Both slots are initially `Empty`. Use `insert_fut1` and `insert_fut2`
    /// to add futures.
    pub fn new() -> Self {
        Self {
            // Safety: An uninitialized `MaybeUninit<T>` is valid.
            fut1: MaybeUninit::uninit(),
            fut2: MaybeUninit::uninit(),
            states: [SlotState::Empty, SlotState::Empty],
        }
    }

    /// Creates a new selector initialized with the two provided futures.
    pub fn with_futures(fut1: Fut1, fut2: Fut2) -> Self {
        Self {
            fut1: MaybeUninit::new(fut1),
            fut2: MaybeUninit::new(fut2),
            states: [SlotState::Occupied, SlotState::Occupied],
        }
    }

    /// Inserts the first future (`Fut1`) into its slot.
    ///
    /// Requires `Pin<&mut Self>` to ensure structural integrity if `Fut1` is `!Unpin`.
    ///
    /// Returns `Ok(())` on success.
    /// Returns `Err(PollerError::SlotOccupied)` if slot 0 is not empty.
    pub fn insert_fut1(self: Pin<&mut Self>, future: Fut1) -> Result<(), PollerError> {
        // Safety: We don't move fields out of `self`.
        let this = unsafe { self.get_unchecked_mut() };

        if this.states[0] != SlotState::Empty {
            return Err(PollerError::SlotOccupied);
        }

        // Write the future into the storage and update the state.
        this.fut1.write(future);
        this.states[0] = SlotState::Occupied;
        Ok(())
    }

    /// Inserts the second future (`Fut2`) into its slot.
    ///
    /// Requires `Pin<&mut Self>` to ensure structural integrity if `Fut2` is `!Unpin`.
    ///
    /// Returns `Ok(())` on success.
    /// Returns `Err(PollerError::SlotOccupied)` if slot 1 is not empty.
    pub fn insert_fut2(self: Pin<&mut Self>, future: Fut2) -> Result<(), PollerError> {
        // Safety: We don't move fields out of `self`.
        let this = unsafe { self.get_unchecked_mut() };

        if this.states[1] != SlotState::Empty {
            return Err(PollerError::SlotOccupied);
        }

        // Write the future into the storage and update the state.
        this.fut2.write(future);
        this.states[1] = SlotState::Occupied;
        Ok(())
    }

    /// Drops the future in the given slot and marks it as Empty.
    ///
    /// # Safety
    /// Caller must ensure `self` is pinned and the slot `index` is `Occupied`.
    unsafe fn drop_future_at(self: Pin<&mut Self>, index: usize) {
        // Safety: We don't move fields out of `self`.
        let this = self.get_unchecked_mut();
        debug_assert!(index < 2 && this.states[index] == SlotState::Occupied);

        match index {
            0 => {
                // Safety: State is Occupied, storage contains a valid Fut1.
                let fut_ptr = this.fut1.as_mut_ptr();
                ptr::drop_in_place(fut_ptr);
            }
            1 => {
                // Safety: State is Occupied, storage contains a valid Fut2.
                let fut_ptr = this.fut2.as_mut_ptr();
                ptr::drop_in_place(fut_ptr);
            }
            _ => unreachable!(),
        }
        this.states[index] = SlotState::Empty;
    }
}

impl<Fut1: Future, Fut2: Future> Future for SelectPin2<Fut1, Fut2> {
    type Output = Either<Fut1::Output, Fut2::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll fut1 if it's occupied
        if self.as_ref().get_ref().states[0] == SlotState::Occupied {
            // Safety: `self` is pinned, state is Occupied.
            let fut1_pin = unsafe {
                // We need `self` pinned *during* the unsafe block.
                let this = self.as_mut().get_unchecked_mut();
                Pin::new_unchecked(this.fut1.assume_init_mut())
            };
            match fut1_pin.poll(cx) {
                Poll::Ready(output) => {
                    // Future completed! Drop it in place and mark slot as empty.
                    // We need `self` pinned to safely drop.
                    // Safety: Future at index 0 just completed, state is Occupied.
                    unsafe { self.as_mut().drop_future_at(0) };
                    return Poll::Ready(Either::First(output));
                }
                Poll::Pending => {}
            }
        }

        // Poll fut2 if it's occupied
        if self.as_ref().get_ref().states[1] == SlotState::Occupied {
            // Safety: `self` is pinned, state is Occupied.
            let fut2_pin = unsafe {
                // We need `self` pinned *during* the unsafe block.
                let this = self.as_mut().get_unchecked_mut();
                Pin::new_unchecked(this.fut2.assume_init_mut())
            };
            match fut2_pin.poll(cx) {
                Poll::Ready(output) => {
                    // Future completed! Drop it in place and mark slot as empty.
                    // Safety: Future at index 1 just completed, state is Occupied.
                    unsafe { self.as_mut().drop_future_at(1) };
                    return Poll::Ready(Either::Second(output));
                }
                Poll::Pending => {}
            }
        }

        // If we reached here, either:
        // - At least one future was polled and returned Pending.
        // - Both slots were Empty initially.
        // In either case, the correct action is to return Pending.
        // The waker logic ensures we'll be polled again if/when something changes.
        Poll::Pending
    }
}

impl<Fut1: Future, Fut2: Future> Drop for SelectPin2<Fut1, Fut2> {
    fn drop(&mut self) {
        // Manually drop any remaining futures.
        if self.states[0] == SlotState::Occupied {
            // Safety: State is Occupied, storage contains a valid Fut1.
            // We are in `drop`, so `self` won't be used again.
            unsafe { ptr::drop_in_place(self.fut1.as_mut_ptr()) };
        }
        if self.states[1] == SlotState::Occupied {
            // Safety: State is Occupied, storage contains a valid Fut2.
            unsafe { ptr::drop_in_place(self.fut2.as_mut_ptr()) };
        }
        // No need to update state, the object is being destroyed.
    }
}

/// Represents the state of a slot in `SelectPin2` or `StaticUnpinPoller`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
enum SlotState {
    /// The slot is empty (future has completed or was never there).
    Empty,
    /// The slot holds an active future being polled.
    Occupied,
}

/// Error type for the StaticUnpinPoller
#[derive(PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub enum PollerError {
    /// Attempted operation on an index that is out of bounds.
    IndexOutOfBounds,
    /// Attempted to insert/replace into a slot that was not empty.
    SlotOccupied,
    /// Attempted to replace/operate on a slot that was not occupied.
    SlotEmpty,
}

/// Polls a fixed number of potentially `!Unpin` futures of the *same type*
/// concurrently without allocation.
///
/// Requires the poller instance itself to be pinned when polling or replacing
/// futures to guarantee memory stability for `!Unpin` types.
pub struct StaticUnpinPoller<F, const N: usize>
where
    F: Future,
{
    // Direct storage for futures. They are never moved once placed here.
    storage: [MaybeUninit<F>; N],
    // Tracks the state of each corresponding slot in `storage`.
    states: [SlotState; N],
}

impl<F, const N: usize> StaticUnpinPoller<F, N>
where
    F: Future,
{
    /// Creates a new, empty poller.
    ///
    /// All slots are initially `Empty`.
    pub fn new() -> Self {
        Self {
            // Safety: An uninitialized `MaybeUninit<T>` is valid.
            storage: array::from_fn(|_| MaybeUninit::uninit()),
            states: [SlotState::Empty; N],
        }
    }

    /// Gets a pinned mutable reference to the future in the slot, if occupied.
    ///
    /// # Safety
    ///
    /// This is the core unsafe operation enabling `!Unpin` support.
    /// It's safe because:
    /// 1. `self` is pinned (`Pin<&mut Self>`), guaranteeing `self.storage` won't move.
    /// 2. We only call `assume_init_mut` when `self.states[index]` is `Occupied`,
    ///    ensuring the `MaybeUninit` contains a valid `F`.
    /// 3. The returned `Pin<&mut F>` points to memory within the pinned `self.storage`,
    ///    and we promise not to move the `F` out of `self.storage[index]` until it's
    ///    dropped via `drop_future_at`.
    unsafe fn get_pin_mut(self: Pin<&mut Self>, index: usize) -> Option<Pin<&mut F>> {
        // Get mutable references to storage and states via the pin projection.
        // `Pin::get_unchecked_mut` is safe because we don't move fields out of `self`.
        let this = self.get_unchecked_mut();
        if this.states[index] == SlotState::Occupied {
            // Safety: We checked the state is Occupied.
            let fut_ref = this.storage[index].assume_init_mut();
            // Safety: The future `fut_ref` is pinned because `self` is pinned,
            // and we won't move it until it's dropped.
            Some(Pin::new_unchecked(fut_ref))
        } else {
            None
        }
    }

    /// Drops the future in the given slot.
    ///
    /// # Safety
    ///
    /// Caller must ensure:
    /// 1. `index` is valid.
    /// 2. `self.states[index]` is `Occupied`.
    /// 3. This is called only once for a given occupied future.
    ///
    /// This function transitions the state to `Empty`.
    unsafe fn drop_future_at(self: Pin<&mut Self>, index: usize) {
        // Safety: We don't move fields out of `self`.
        let this = self.get_unchecked_mut();
        debug_assert!(this.states[index] == SlotState::Occupied);

        // Safety: State is Occupied, so `storage[index]` contains a valid F
        // that needs to be dropped.
        let fut_ptr = this.storage[index].as_mut_ptr();
        ptr::drop_in_place(fut_ptr);

        // Mark as empty now that it's dropped
        this.states[index] = SlotState::Empty;
    }

    /// Inserts a future into an empty slot.
    ///
    /// Requires `Pin<&mut Self>` to ensure structural integrity if `F` is `!Unpin`,
    /// although technically not strictly needed just for insertion if the poller
    /// hasn't been polled yet. Consistent API is preferred.
    ///
    /// Returns `Ok(())` on success.
    /// Returns `Err(PollerError::IndexOutOfBounds)` if the index is invalid.
    /// Returns `Err(PollerError::SlotOccupied)` if the slot is not empty.
    pub fn insert(mut self: Pin<&mut Self>, future: F) -> Result<(), PollerError> {
        // Safety: We don't move fields out of `self`.
        let this = unsafe { self.as_mut().get_unchecked_mut() };

        let (index, state) = this
            .states
            .iter_mut()
            .enumerate()
            .find(|(_, state)| **state == SlotState::Empty)
            .ok_or(PollerError::IndexOutOfBounds)?;

        // Write the future into the storage and update the state.
        this.storage[index].write(future);
        *state = SlotState::Occupied;
        Ok(())
    }

    /// Replaces the future in a slot, assuming it was previously occupied and completed.
    ///
    /// This is intended to be called after `poll_next` returns `Poll::Ready(Some((index, _)))`.
    /// The slot associated with `index` should have been implicitly emptied by `poll_next`.
    ///
    /// Returns `Ok(())` on success.
    /// Returns `Err(PollerError::IndexOutOfBounds)` if the index is invalid.
    /// Returns `Err(PollerError::SlotOccupied)` if the slot is not currently empty
    /// (e.g., `poll_next` didn't complete for this index, or called incorrectly).
    pub fn replace(
        mut self: Pin<&mut Self>,
        index: usize,
        new_future: F,
    ) -> Result<(), PollerError> {
        // Safety: We don't move fields out of `self`.
        let this = unsafe { self.as_mut().get_unchecked_mut() };

        let state = this
            .states
            .get_mut(index)
            .ok_or(PollerError::IndexOutOfBounds)?;

        // After poll_next completes a future, the slot state becomes Empty.
        if *state != SlotState::Empty {
            // This indicates a logic error: replacing before completion or replacing the wrong index.
            return Err(PollerError::SlotOccupied);
        }

        // Write the new future and mark as occupied.
        this.storage[index].write(new_future);
        *state = SlotState::Occupied;
        Ok(())
    }

    /// Polls the set of futures and returns the result of the first one to complete.
    ///
    /// Requires `Pin<&mut Self>` to safely poll potentially `!Unpin` futures.
    ///
    /// Returns `Poll::Ready(Some((index, output)))` when a future completes.
    /// The slot at `index` is automatically dropped and marked as `Empty`.
    ///
    /// Returns `Poll::Ready(None)` if all slots are currently empty.
    ///
    /// Returns `Poll::Pending` if no future is ready, but at least one is pending.
    /// The context's waker will be registered for all pending futures.
    pub fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<(usize, F::Output)>> {
        let mut pending_found = false;
        let mut occupied_count = 0;

        // We need to iterate carefully, as polling might modify `self.states`.
        for index in 0..N {
            // Check state *before* potentially getting a pinned reference.
            // We need `self` pinned *during* the unsafe block.
            let current_state = self.as_ref().get_ref().states[index];

            if current_state == SlotState::Occupied {
                occupied_count += 1;

                // Safety: `self` is pinned, state is Occupied. We get a valid Pin<&mut F>.
                let pinned_fut = unsafe { self.as_mut().get_pin_mut(index) }
                    .expect("State mismatch: Expected Occupied but get_pin_mut failed"); // Should not happen

                match pinned_fut.poll(cx) {
                    Poll::Ready(output) => {
                        // Future completed! Drop it in place and mark slot as empty.
                        // We need `self` pinned to safely drop.
                        // Safety: Future at `index` just completed, state is Occupied.
                        unsafe { self.as_mut().drop_future_at(index) };

                        return Poll::Ready(Some((index, output)));
                    }
                    Poll::Pending => {
                        // Future is not ready yet. Waker registered by poll.
                        pending_found = true;
                    }
                }
            }
        } // End loop

        if occupied_count == 0 {
            // No futures were present in any slot.
            Poll::Ready(None)
        } else if pending_found {
            // At least one future was polled and is pending.
            Poll::Pending
        } else {
            // All occupied slots were polled, but none were Ready and none were Pending.
            // This implies all occupied futures completed *simultaneously* in a previous
            // poll, but we only returned one. The remaining slots are Occupied but finished.
            // Polling them again might not make progress.
            // However, a valid Future should always return Pending if not Ready.
            // This state *shouldn't* be reachable with correct Future impls.
            // For robustness, treat as Pending, assuming wakers might fire later
            // if the Futures have strange final states.
            Poll::Pending
        }
    }

    /// Returns the number of futures currently occupying slots.
    pub fn len(&self) -> usize {
        self.states
            .iter()
            .filter(|&&s| s == SlotState::Occupied)
            .count()
    }

    /// Checks if all slots are empty.
    pub fn is_empty(&self) -> bool {
        self.states.iter().all(|&s| s == SlotState::Empty)
    }
}

impl<F: Future, const N: usize> Future for StaticUnpinPoller<F, N> {
    type Output = Option<(usize, F::Output)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.poll_next(cx)
    }
}

impl<F: Future, const N: usize> Drop for StaticUnpinPoller<F, N> {
    fn drop(&mut self) {
        // Manually drop any remaining futures in occupied slots.
        for i in 0..N {
            if self.states[i] == SlotState::Occupied {
                // Safety: State is Occupied, storage contains a valid F.
                // We are in `drop`, so `self` won't be used again, making it safe
                // to get a mutable pointer and drop in place. Pinning is not
                // strictly required here as the object is being destroyed.
                unsafe {
                    let fut_ptr = self.storage[i].as_mut_ptr();
                    ptr::drop_in_place(fut_ptr);
                    // No need to update state, the whole object is dying.
                }
            }
        }
    }
}
