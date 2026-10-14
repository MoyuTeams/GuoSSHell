//! 实际已呈现几何的冻结租约；准入和原生去重与 key 共用 port 临界区。
use super::{
    Admission, Arc, EVENT_BYTES, EffectKind, PublishedProjection, Reject, Route, Surface, TextOp,
    Ticket, enqueue, refund, reserve, transactions, valid, valid_binding,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PresentedGeometry {
    pub frame_revision: u32,
    pub row_revision: u64,
    pub viewport_top: i64,
    pub font_revision: u64,
    pub layout_revision: u64,
    pub transform_revision: u64,
    pub clip_x: f64,
    pub clip_y: f64,
    pub clip_width: f64,
    pub clip_height: f64,
    pub transform: [f64; 6],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PointerInput {
    pub geometry: PresentedGeometry,
    /// kind: 1=press，2=move，3=release，4=wheel；button: 0=none，1=left，2=middle，3=right。
    pub kind: u32,
    pub button: u32,
    pub modifiers: u32,
    pub buttons: u32,
    pub stable_hit_row: i64,
    pub column: u32,
    pub remote_column: u32,
    pub remote_row: u32,
    /// 按行计，正数向下；保留原请求，不按新的几何重算。
    pub delta_x: i32,
    pub delta_y: i32,
}

pub(crate) struct FrozenPointer {
    pub input: PointerInput,
    pub ticket: Ticket,
    pub presented: Arc<PublishedProjection>,
}

impl Surface {
    pub(crate) fn invalidate_geometry(&self, request_id: u64) -> Result<(), Reject> {
        self.on_ui_thread()?;
        if self
            .pointer_pending
            .try_lock()
            .map_err(|_| Reject::Busy)?
            .is_some()
        {
            return Err(Reject::Busy);
        }
        let mut state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
        valid_binding(&state, self.binding)?;
        if state.busy_pointer.is_some() {
            return Err(Reject::Busy);
        }
        if request_id <= state.last_native_request {
            return Err(Reject::Duplicate);
        }
        let old = state.presented.take();
        state.last_native_request = request_id;
        drop(state);
        drop(old);
        Ok(())
    }
    pub(crate) fn presented_geometry(
        &self,
        request_id: u64,
        geometry: PresentedGeometry,
    ) -> Result<(), Reject> {
        self.on_ui_thread()?;
        if self
            .pointer_pending
            .try_lock()
            .map_err(|_| Reject::Busy)?
            .is_some()
        {
            return Err(Reject::Busy);
        }
        if ![
            geometry.clip_x,
            geometry.clip_y,
            geometry.clip_width,
            geometry.clip_height,
        ]
        .iter()
        .chain(geometry.transform.iter())
        .all(|n| n.is_finite())
            || geometry.clip_width <= 0.0
            || geometry.clip_height <= 0.0
            || geometry.font_revision == 0
            || geometry.layout_revision == 0
            || geometry.transform_revision == 0
        {
            return Err(Reject::InvalidEvent);
        }
        let mut state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
        valid_binding(&state, self.binding)?;
        if state.busy_pointer.is_some() {
            return Err(Reject::Busy);
        }
        if request_id <= state.last_native_request {
            return Err(Reject::Duplicate);
        }
        let projection = state.projection.as_ref().ok_or(Reject::NotAttached)?;
        if geometry.frame_revision != projection.frame_revision
            || geometry.row_revision != projection.row_revision
            || geometry.viewport_top != projection.frame().viewport_top
        {
            return Err(Reject::InvalidEvent);
        }
        let new = (geometry, projection.clone());
        let old = state.presented.replace(new);
        state.last_native_request = request_id;
        drop(state);
        drop(old);
        Ok(())
    }

    pub(crate) fn pointer(
        &self,
        request_id: u64,
        input: PointerInput,
    ) -> Result<Admission, Reject> {
        self.on_ui_thread()?;
        let mut pending = self.pointer_pending.try_lock().map_err(|_| Reject::Busy)?;
        if pending.is_some_and(|original| original != (request_id, input)) {
            return Err(Reject::Busy);
        }
        *pending = Some((request_id, input));
        let result = self.admit_pointer(request_id, input);
        if result != Err(Reject::Busy) {
            *pending = None;
        }
        result
    }

    fn admit_pointer(&self, request_id: u64, input: PointerInput) -> Result<Admission, Reject> {
        let mut state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
        valid(&state, self.binding)?;
        if state.busy_pointer.is_some_and(|id| id != request_id) {
            return Err(Reject::Busy);
        }
        if request_id <= state.last_native_request {
            return Err(Reject::Duplicate);
        }
        let (geometry, projection) = state.presented.as_ref().ok_or(Reject::NotAttached)?;
        if *geometry != input.geometry
            || !(1..=4).contains(&input.kind)
            || input.button > 3
            || input.modifiers & !15 != 0
            || input.buttons & !7 != 0
            || input.column >= u32::from(projection.frame().size.cols)
            || input.remote_column >= u32::from(projection.frame().size.cols)
            || input.remote_row >= u32::from(projection.frame().size.rows)
            || !projection
                .frame()
                .rows
                .iter()
                .any(|row| row.stable_row == input.stable_hit_row)
            || input.delta_x.unsigned_abs() > 64
            || input.delta_y.unsigned_abs() > 64
            || input.kind != 4 && (input.delta_x != 0 || input.delta_y != 0)
            || input.kind == 4 && input.delta_x != 0
        {
            return Err(Reject::InvalidEvent);
        }
        let projection = projection.clone();
        let bytes = EVENT_BYTES + std::mem::size_of::<FrozenPointer>();
        if let Err(error) = reserve(&mut state, bytes, true) {
            if error == Reject::Busy {
                state.busy_pointer = Some(request_id);
            }
            return Err(error);
        }
        if !self.port.mode.native() {
            refund(&mut state, bytes, true);
            return Err(Reject::MixedMode);
        }
        let ticket = Ticket {
            registration: self.port.registration,
            binding: self.binding,
            cycle: 0,
            transaction: 0,
            context: 0,
        };
        let ended = self.port.seal_text(&mut state);
        let end = state
            .texts
            .iter()
            .flatten()
            .find_map(|text| match text.text {
                transactions::TextState::Ending(end) if ended.is_some() => Some(end),
                _ => None,
            });
        let effect = enqueue(
            &self.port,
            &mut state,
            ticket,
            EffectKind::Pointer(FrozenPointer {
                input,
                ticket,
                presented: projection,
            }),
            bytes,
            true,
        );
        state.last_native_request = request_id;
        state.busy_pointer = None;
        drop(state);
        drop(ended);
        self.port.wake.notify_waiters();
        Ok(Admission {
            ticket,
            effect,
            route: Route::Terminal,
            cycle_id: 0,
            text_op: if end.is_some() {
                TextOp::End
            } else {
                TextOp::None
            },
            end,
        })
    }
}
