//! Boundary wrappers around the [`shared`] Baseline solving logic.

use crate::{
    boundary,
    domain::{baseline, eth, liquidity, order},
};
use ethereum_types::{H160, U256};
use model::TokenPair;
use shared::baseline_solver::{self, BaseTokens, BaselineSolvable};
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroUsize,
};

pub struct Solver<'a> {
    base_tokens: BaseTokens,
    amms: HashMap<TokenPair, Vec<Amm>>,
    liquidity: HashMap<liquidity::Id, &'a liquidity::Liquidity>,
}

impl<'a> Solver<'a> {
    pub fn new(
        weth: &eth::WethAddress,
        base_tokens: &HashSet<eth::TokenAddress>,
        liquidity: &'a [liquidity::Liquidity],
    ) -> Self {
        Self {
            base_tokens: to_boundary_base_tokens(weth, base_tokens),
            amms: to_boundary_amms(liquidity),
            liquidity: liquidity
                .iter()
                .map(|liquidity| (liquidity.id.clone(), liquidity))
                .collect(),
        }
    }

    pub fn route(
        &self,
        order: order::UserOrder,
        max_hops: NonZeroUsize,
    ) -> Option<baseline::Route<'a>> {
        let candidates = self.base_tokens.path_candidates_with_hops(
            order.get().sell.token.0,
            order.get().buy.token.0,
            max_hops,
        );

        let order = order.get();
        let (path, executed_sell_amount) = match order.side {
            order::Side::Buy => {
                let best = candidates
                    .iter()
                    .filter_map(|path| {
                        baseline_solver::estimate_sell_amount(order.buy.amount, path, &self.amms)
                    })
                    .filter(|estimate| estimate.value <= order.sell.amount)
                    .min_by_key(|estimate| estimate.value)?;
                (best.path, best.value)
            }
            order::Side::Sell => {
                let best = candidates
                    .iter()
                    .filter_map(|path| {
                        baseline_solver::estimate_buy_amount(order.sell.amount, path, &self.amms)
                    })
                    .filter(|estimate| estimate.value >= order.buy.amount)
                    .max_by_key(|estimate| estimate.value)?;
                (best.path, order.sell.amount)
            }
        };

        baseline::Route::new(self.traverse_path(&path, order.sell.token.0, executed_sell_amount)?)
    }

    fn traverse_path(
        &self,
        path: &[&Amm],
        mut sell_token: H160,
        mut sell_amount: U256,
    ) -> Option<Vec<baseline::Segment<'a>>> {
        let mut segments = Vec::new();
        for liquidity in path {
            let reference_liquidity = self
                .liquidity
                .get(&liquidity.id)
                .expect("boundary liquidity does not match ID");

            let buy_token = liquidity
                .token_pair
                .other(&sell_token)
                .expect("Inconsistent path");
            let buy_amount = liquidity.get_amount_out(buy_token, (sell_amount, sell_token))?;

            segments.push(baseline::Segment {
                liquidity: reference_liquidity,
                input: eth::Asset {
                    token: eth::TokenAddress(sell_token),
                    amount: sell_amount,
                },
                output: eth::Asset {
                    token: eth::TokenAddress(buy_token),
                    amount: buy_amount,
                },
            });

            sell_token = buy_token;
            sell_amount = buy_amount;
        }
        Some(segments)
    }
}

fn to_boundary_amms(liquidity: &[liquidity::Liquidity]) -> HashMap<TokenPair, Vec<Amm>> {
    liquidity
        .iter()
        .fold(HashMap::new(), |mut amms, liquidity| {
            match &liquidity.state {
                liquidity::State::ConstantProduct(pool) => {
                    let boundary_pool = boundary::liquidity::constantproduct::to_boundary_pool(
                        liquidity.address,
                        pool,
                    );
                    amms.entry(boundary_pool.tokens).or_default().push(Amm {
                        id: liquidity.id.clone(),
                        token_pair: boundary_pool.tokens,
                        pool: Pool::ConstantProduct(boundary_pool),
                    });
                }
                liquidity::State::WeightedProduct(pool) => {
                    let boundary_pool =
                        boundary::liquidity::weighted::to_boundary_pool(liquidity.address, pool);
                    for pair in pool.token_pairs() {
                        let token_pair = to_boundary_token_pair(&pair);
                        amms.entry(token_pair).or_default().push(Amm {
                            id: liquidity.id.clone(),
                            token_pair,
                            pool: Pool::Weighted(boundary_pool.clone()),
                        });
                    }
                }
                // Other liquidity types are not supported...
                _ => {}
            };
            amms
        })
}

struct Amm {
    id: liquidity::Id,
    token_pair: TokenPair,
    pool: Pool,
}

enum Pool {
    ConstantProduct(boundary::liquidity::constantproduct::Pool),
    Weighted(boundary::liquidity::weighted::Pool),
}

impl BaselineSolvable for Amm {
    fn get_amount_out(&self, out_token: H160, input: (U256, H160)) -> Option<U256> {
        match &self.pool {
            Pool::ConstantProduct(pool) => pool.get_amount_out(out_token, input),
            Pool::Weighted(pool) => pool.get_amount_out(out_token, input),
        }
    }

    fn get_amount_in(&self, in_token: H160, out: (U256, H160)) -> Option<U256> {
        match &self.pool {
            Pool::ConstantProduct(pool) => pool.get_amount_in(in_token, out),
            Pool::Weighted(pool) => pool.get_amount_in(in_token, out),
        }
    }

    fn gas_cost(&self) -> usize {
        match &self.pool {
            Pool::ConstantProduct(pool) => pool.gas_cost(),
            Pool::Weighted(pool) => pool.gas_cost(),
        }
    }
}

fn to_boundary_base_tokens(
    weth: &eth::WethAddress,
    base_tokens: &HashSet<eth::TokenAddress>,
) -> BaseTokens {
    let base_tokens = base_tokens.iter().map(|token| token.0).collect::<Vec<_>>();
    BaseTokens::new(weth.0, &base_tokens)
}

fn to_boundary_token_pair(pair: &eth::TokenPair) -> TokenPair {
    let (a, b) = pair.get();
    TokenPair::new(a.0, b.0).unwrap()
}