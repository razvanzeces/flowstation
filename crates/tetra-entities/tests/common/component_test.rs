use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use tetra_config::bluestation::{SharedConfig, StackConfig, StackMode};
use tetra_core::TdmaTime;
use tetra_core::tetra_entities::TetraEntity;
use tetra_entities::{MessageRouter, TetraEntityTrait};
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

// BS imports
use tetra_entities::cmce::cmce_bs::CmceBs;
use tetra_entities::cmce::cmce_ms::CmceMs;
use tetra_entities::llc::llc_bs_ms::Llc;
use tetra_entities::lmac::lmac_bs::LmacBs;
use tetra_entities::mle::mle_bs::MleBs;
use tetra_entities::mm::mm_bs::MmBs;
use tetra_entities::sndcp::sndcp_bs::Sndcp;
use tetra_entities::umac::umac_bs::UmacBs;

// MS imports
use tetra_entities::lmac::lmac_ms::LmacMs;
use tetra_entities::umac::umac_ms::UmacMs;

use crate::common::default_stack;

use super::sink::Sink;

thread_local! {
    /// Fingerprint of what `dump_sinks` returned on this thread since `trace_arm`.
    static TRACE: RefCell<Option<DefaultHasher>> = const { RefCell::new(None) };
    /// Applied, on this thread, to every configuration a `ComponentTest` is built from.
    static CONFIG_TWEAK: Cell<Option<fn(&mut StackConfig)>> = const { Cell::new(None) };
    /// The slots of this carrier are left out of the fingerprint on this thread.
    static TRACE_SKIP_CARRIER: Cell<Option<u16>> = const { Cell::new(None) };
}

/// Run `f` with the downlink slots of `carrier` left out of the fingerprint on this thread (a
/// scenario run again with one more carrier, for the identity tests).
pub fn with_trace_skip_carrier<R>(carrier: u16, f: impl FnOnce() -> R) -> R {
    TRACE_SKIP_CARRIER.with(|c| c.set(Some(carrier)));
    let out = f();
    TRACE_SKIP_CARRIER.with(|c| c.set(None));
    out
}

/// Run `f` with `tweak` applied to every configuration a `ComponentTest` is built from on this
/// thread (a scenario run again with one more key, for the identity tests).
pub fn with_config_tweak<R>(tweak: fn(&mut StackConfig), f: impl FnOnce() -> R) -> R {
    CONFIG_TWEAK.with(|t| t.set(Some(tweak)));
    let out = f();
    CONFIG_TWEAK.with(|t| t.set(None));
    out
}

/// Start a fingerprint, on this thread, of every slot the MAC puts on the air and every PDU
/// passed between the MAC and the LLC that `dump_sinks` returns from now on.
pub fn trace_arm() {
    TRACE.with(|t| *t.borrow_mut() = Some(DefaultHasher::new()));
}

/// The fingerprint so far (0 when not armed).
pub fn trace_hash() -> u64 {
    TRACE.with(|t| t.borrow().as_ref().map_or(0, |h| h.finish()))
}

/// Feed the fingerprint, when armed, with the Debug form of `msgs` (sorted: entities keep some
/// state in hash maps, so the order within one tick changes from run to run). The TMA data
/// category of a packet-data channel assignment (`DATA_CATEGORY_PDCH_ASSIGNMENT`), which came
/// after the fingerprints were recorded and never goes on the air, is left out, and so are the
/// slots of the carrier `with_trace_skip_carrier` names.
fn trace(msgs: &[SapMsg]) {
    let skip = TRACE_SKIP_CARRIER.with(Cell::get);
    TRACE.with(|t| {
        if let Some(h) = t.borrow_mut().as_mut() {
            let mut lines: Vec<String> = msgs
                .iter()
                .filter(|m| {
                    matches!(
                        m.msg,
                        SapMsgInner::TmvUnitdataReqSlots(_) | SapMsgInner::TmaUnitdataInd(_) | SapMsgInner::TmaUnitdataReq(_)
                    )
                })
                .map(|m| match (&m.msg, skip) {
                    (SapMsgInner::TmvUnitdataReqSlots(slots), Some(carrier)) => {
                        let mut kept = slots.clone();
                        kept.slots.retain(|s| s.carrier_num != carrier);
                        format!("{:?}", SapMsgInner::TmvUnitdataReqSlots(kept))
                    }
                    _ => format!("{:?}", m.msg),
                })
                .map(|line| line.replace("data_category: Some(2)", "data_category: None"))
                .collect();
            lines.sort();
            lines.hash(h);
        }
    });
}

/// Infrastructure for testing TETRA components
/// Quick setup of all components for end-to-end testing
/// Supports optional sinks for collecting messages for later inspection
pub struct ComponentTest {
    pub config: SharedConfig,
    pub router: MessageRouter,
    // components: Vec<TetraEntity>,
    pub sinks: Vec<TetraEntity>,
    start_dl_time: TdmaTime,
}

impl ComponentTest {
    /// Get a default test config for the given stack mode, suitable for testing
    /// stack components. The config can be further modified if needed before creating the ComponentTest instance.
    pub fn get_default_test_config(stack_mode: StackMode) -> StackConfig {
        match stack_mode {
            StackMode::Bs => default_stack::default_test_config_bs(),
            StackMode::Ms => default_stack::default_test_config_ms(),
            _ => panic!("Unsupported stack mode"),
        }
    }

    /// Create a new ComponentTest instance with the given config and optional start downlink time.
    pub fn from_config(mut config: StackConfig, start_dl_time: Option<TdmaTime>) -> Self {
        if let Some(tweak) = CONFIG_TWEAK.with(Cell::get) {
            tweak(&mut config);
        }
        let shared_config = SharedConfig::from_parts(config, None);
        let config_clone = shared_config.clone();
        let mut mr = MessageRouter::new(config_clone);

        let start_dl_time = start_dl_time.unwrap_or_default();
        mr.set_dl_time(start_dl_time);

        Self {
            config: shared_config,
            router: mr,
            sinks: vec![],
            start_dl_time,
        }
    }

    /// Create a new ComponentTest instance with a default config for the given stack mode.
    /// Optionally specify a start downlink time.
    pub fn new(stack_mode: StackMode, start_dl_time: Option<TdmaTime>) -> Self {
        let config = Self::get_default_test_config(stack_mode);
        Self::from_config(config, start_dl_time)
    }

    /// Get a cloned copy of the ComponentTest's shared config
    pub fn get_shared_config(&self) -> SharedConfig {
        self.config.clone()
    }

    /// Automatically add a set of components and sinks to the stack. Depending on the stack mode,
    /// the components will be created with the appropriate BS or MS implementations.
    pub fn populate_entities(&mut self, components: Vec<TetraEntity>, sinks: Vec<TetraEntity>) {
        match self.config.config().stack_mode {
            StackMode::Bs => {
                self.create_components_bs(components);
            }
            StackMode::Ms => {
                self.create_components_ms(components);
            }
            _ => {
                panic!("Only BS stack mode is supported in ComponentTest");
            }
        }

        // Create sinks for debugging / message collection
        self.create_sinks(sinks);
    }

    fn create_components_bs(&mut self, components: Vec<TetraEntity>) {
        // Setup the stack with all requested components, performing set-up where needed
        for component in components.iter() {
            match component {
                TetraEntity::Lmac => {
                    let lmac = LmacBs::new(self.config.clone());
                    self.register_entity(lmac);
                }
                TetraEntity::Umac => {
                    let umac = UmacBs::new(self.config.clone(), None);
                    // Do not hand-prime the scheduler clock: the first tick_start auto-initialises the
                    // primary AND every secondary-carrier scheduler to the real time (umac_bs tick_start
                    // init gate). Hand-priming only the primary left secondaries un-synced and also
                    // bypassed that production path. Letting it run mirrors live BS startup exactly.
                    self.router.register_entity(Box::new(umac));
                }
                TetraEntity::Llc => {
                    let llc = Llc::new(self.config.clone());
                    self.router.register_entity(Box::new(llc));
                }
                TetraEntity::Mle => {
                    let mle = MleBs::new(self.config.clone());
                    self.router.register_entity(Box::new(mle));
                }
                TetraEntity::Mm => {
                    let mm = MmBs::new(self.config.clone(), None, None);
                    self.router.register_entity(Box::new(mm));
                }
                TetraEntity::Sndcp => {
                    let sndcp = Sndcp::new(self.config.clone());
                    self.router.register_entity(Box::new(sndcp));
                }
                TetraEntity::Cmce => {
                    let cmce = CmceBs::new(self.config.clone(), None, None);
                    self.router.register_entity(Box::new(cmce));
                }
                _ => {
                    panic!("Component not implemented: {:?}", component);
                }
            }
        }
    }

    fn create_components_ms(&mut self, components: Vec<TetraEntity>) {
        for component in components.iter() {
            match component {
                TetraEntity::Lmac => {
                    let lmac = LmacMs::new(self.config.clone());
                    self.router.register_entity(Box::new(lmac));
                }
                TetraEntity::Umac => {
                    let umac = UmacMs::new(self.config.clone());
                    self.router.register_entity(Box::new(umac));
                }
                TetraEntity::Llc => {
                    let llc = Llc::new(self.config.clone());
                    self.router.register_entity(Box::new(llc));
                }
                TetraEntity::Mle => {
                    let mle = MleBs::new(self.config.clone());
                    self.router.register_entity(Box::new(mle));
                }
                TetraEntity::Cmce => {
                    let cmce = CmceMs::new(self.config.clone());
                    self.router.register_entity(Box::new(cmce));
                }
                _ => {
                    panic!("Component not implemented: {:?}", component);
                }
            }
        }
    }

    fn create_sinks(&mut self, sinks: Vec<TetraEntity>) {
        // Setup any sinks
        for sink in sinks.iter() {
            assert!(!self.sinks.contains(sink), "Sink already exists: {:?}", sink);
            assert!(
                self.router.get_entity(*sink).is_none(),
                "Sink already registered as entity: {:?}",
                sink
            );

            self.sinks.push(*sink);
            let sink = Sink::new(*sink);
            self.router.register_entity(Box::new(sink));
        }
    }

    pub fn register_entity<T: 'static + TetraEntityTrait>(&mut self, entity: T) {
        self.router.register_entity(Box::new(entity));
    }

    pub fn run_stack(&mut self, num_ticks: Option<usize>) {
        self.router.run_stack(num_ticks, None);
    }

    pub fn submit_message(&mut self, message: SapMsg) {
        self.router.submit_message(message);
    }

    pub fn deliver_all_messages(&mut self) {
        self.router.deliver_all_messages();
    }

    pub fn dump_sinks(&mut self) -> Vec<SapMsg> {
        let mut msgs = vec![];
        for sink in self.sinks.iter() {
            if let Some(component) = self.router.get_entity(*sink)
                && let Some(sink) = component.as_any_mut().downcast_mut::<Sink>()
            {
                let mut sink_msgs = sink.take_msgqueue();
                msgs.append(&mut sink_msgs);
            }
        }
        trace(&msgs);
        msgs
    }
}
