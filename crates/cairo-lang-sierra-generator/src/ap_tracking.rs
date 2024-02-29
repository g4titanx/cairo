use cairo_lang_lowering::borrow_check::analysis::{Analyzer, BackAnalysis, StatementLocation};
use cairo_lang_lowering::{
    BlockId, FlatLowered, MatchInfo, Statement, VarRemapping, VarUsage, VariableId,
};
use cairo_lang_utils::ordered_hash_map::OrderedHashMap;
use cairo_lang_utils::ordered_hash_set::OrderedHashSet;
use cairo_lang_utils::unordered_hash_set::UnorderedHashSet;
use itertools::{izip, Itertools};

/// Information about where AP tracking should be enabled/disabled.
#[derive(Default)]
pub struct ApTrackingConfiguration {
    /// Blocks where ap tracking should be enabled.
    pub enable_ap_tracking: UnorderedHashSet<BlockId>,

    /// Blocks where ap tracking should be disabled.
    pub disable_ap_tracking: UnorderedHashSet<BlockId>,
}

/// Collects information about where ap tracking should be enabled/disabled.
pub fn get_ap_tracking_configuration(
    lowered_function: &FlatLowered,
    known_ap_change: bool,
    vars_of_interest: OrderedHashSet<VariableId>,
) -> ApTrackingConfiguration {
    let mut ctx = ApTrackingAnalysisContext {
        vars_of_interest,
        ap_tracking_configuration: ApTrackingConfiguration {
            enable_ap_tracking: UnorderedHashSet::default(),
            disable_ap_tracking: UnorderedHashSet::default(),
        },
    };

    if ctx.vars_of_interest.is_empty() {
        if !known_ap_change {
            ctx.ap_tracking_configuration.disable_ap_tracking.insert(BlockId::root());
        }

        return ctx.ap_tracking_configuration;
    }

    let mut analysis =
        BackAnalysis { lowered: lowered_function, block_info: Default::default(), analyzer: ctx };
    analysis.get_root_info();

    analysis.analyzer.ap_tracking_configuration
}

/// Context for the ap tracking analysis.
/// This analysis is used to determine where ap tracking should be enabled/disabled
/// based on `vars_of_interest`
struct ApTrackingAnalysisContext {
    /// The variables that require ap alignment.
    pub vars_of_interest: OrderedHashSet<VariableId>,

    /// The configuration that is generated by the analysis.
    pub ap_tracking_configuration: ApTrackingConfiguration,
}

/// The info struct for the ap tracking analysis.
#[derive(Clone)]
struct ApTrackingAnalysisInfo {
    /// A mapping from variables to the blocks where they are used.
    vars: OrderedHashMap<VariableId, OrderedHashSet<BlockId>>,
}

impl ApTrackingAnalysisInfo {
    pub fn variables_used<'a>(
        &mut self,
        ctx: &ApTrackingAnalysisContext,
        vars: impl Iterator<Item = &'a VariableId>,
        block_id: BlockId,
    ) {
        for var_id in vars {
            if !ctx.vars_of_interest.contains(var_id) {
                continue;
            }
            self.vars.entry(*var_id).or_default().insert(block_id);
        }
    }
}

impl Analyzer<'_> for ApTrackingAnalysisContext {
    type Info = ApTrackingAnalysisInfo;

    fn visit_stmt(
        &mut self,
        info: &mut Self::Info,
        (block_id, _statement_index): StatementLocation,
        stmt: &Statement,
    ) {
        for var_id in stmt.outputs() {
            info.vars.swap_remove(var_id);
        }

        info.variables_used(
            self,
            stmt.inputs().iter().map(|VarUsage { var_id, .. }| var_id),
            block_id,
        );
    }

    fn visit_goto(
        &mut self,
        info: &mut Self::Info,
        (block_id, _statement_index): StatementLocation,
        _target_block_id: BlockId,
        remapping: &VarRemapping,
    ) {
        for dst in remapping.keys() {
            info.vars.swap_remove(dst);
        }

        // If none of the variable is alive after the convergence then we can disable ap tracking.
        if info.vars.is_empty() {
            self.ap_tracking_configuration.disable_ap_tracking.insert(block_id);
        }

        info.variables_used(
            self,
            remapping.values().map(|VarUsage { var_id, .. }| var_id),
            block_id,
        );
    }

    fn merge_match(
        &mut self,
        (block_id, _statement_index): StatementLocation,
        match_info: &MatchInfo,
        infos: &[Self::Info],
    ) -> Self::Info {
        // Find all the variables that are alive after this block convergence.
        // A variable is alive after a converges if it is a alive in some block that is reachable
        // from it by any arm.
        let mut vars = OrderedHashMap::<VariableId, OrderedHashSet<BlockId>>::default();
        let mut any_merges = false;
        for (arm, info) in izip!(match_info.arms(), infos) {
            for (var_id, blocks) in info.vars.iter() {
                if arm.var_ids.iter().contains(var_id) {
                    continue;
                }
                let var_blocks = vars.entry(*var_id).or_default();
                for block_id in blocks {
                    any_merges |= !var_blocks.insert(*block_id);
                }
            }
        }

        // If we have a variable the lives in more then one arm we need to enable ap tracking
        // If there are no variables of interest in any of the arms, we can disable ap tracking.
        if any_merges {
            self.ap_tracking_configuration.enable_ap_tracking.insert(block_id);
        } else if vars.is_empty() {
            self.ap_tracking_configuration.disable_ap_tracking.insert(block_id);
        }

        let mut info = Self::Info { vars };
        info.variables_used(
            self,
            match_info.inputs().iter().map(|VarUsage { var_id, .. }| var_id),
            block_id,
        );
        info
    }

    fn info_from_return(
        &mut self,
        (block_id, _statement_index): StatementLocation,
        vars: &[VarUsage],
    ) -> Self::Info {
        // TODO(ilya): Consider the following disabling of ap tracking.

        // Since the function has an unknown ap change we need to disable ap tracking
        // before any return.
        self.ap_tracking_configuration.disable_ap_tracking.insert(block_id);

        let mut info = Self::Info { vars: Default::default() };
        info.variables_used(self, vars.iter().map(|VarUsage { var_id, .. }| var_id), block_id);
        info
    }
}
