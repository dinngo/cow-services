//! A 0x-based trade finder.

use super::{Interaction, Trade, TradeError, TradeFinding};
use crate::{
    price_estimation::{gas, Query},
    zeroex_api::{SwapQuery, ZeroExApi, ZeroExResponseError},
};
use model::order::OrderKind;
use std::sync::Arc;

pub struct ZeroExTradeFinder {
    api: Arc<dyn ZeroExApi>,
    excluded_sources: Vec<String>,
}

impl ZeroExTradeFinder {
    pub fn new(api: Arc<dyn ZeroExApi>, excluded_sources: Vec<String>) -> Self {
        Self {
            api,
            excluded_sources,
        }
    }

    async fn quote(&self, query: &Query) -> Result<Trade, TradeError> {
        let (sell_amount, buy_amount) = match query.kind {
            OrderKind::Buy => (None, Some(query.in_amount)),
            OrderKind::Sell => (Some(query.in_amount), None),
        };

        let swap = self
            .api
            .get_swap(SwapQuery {
                sell_token: query.sell_token,
                buy_token: query.buy_token,
                sell_amount,
                buy_amount,
                slippage_percentage: Default::default(),
                excluded_sources: self.excluded_sources.clone(),
                enable_slippage_protection: false,
            })
            .await?;

        Ok(Trade {
            out_amount: match query.kind {
                OrderKind::Buy => swap.price.sell_amount,
                OrderKind::Sell => swap.price.buy_amount,
            },
            gas_estimate: gas::SETTLEMENT_OVERHEAD + swap.price.estimated_gas,
            approval_spender: Some(swap.price.allowance_target),
            interaction: Interaction {
                target: swap.to,
                value: swap.value,
                data: swap.data,
            },
        })
    }
}

#[async_trait::async_trait]
impl TradeFinding for ZeroExTradeFinder {
    async fn get_trade(&self, query: &Query) -> Result<Trade, TradeError> {
        self.quote(query).await
    }
}

impl From<ZeroExResponseError> for TradeError {
    fn from(err: ZeroExResponseError) -> Self {
        TradeError::Other(err.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zeroex_api::{DefaultZeroExApi, PriceResponse};
    use crate::zeroex_api::{MockZeroExApi, SwapResponse};
    use reqwest::Client;

    fn create_trader(api: Arc<dyn ZeroExApi>) -> ZeroExTradeFinder {
        ZeroExTradeFinder {
            api,
            excluded_sources: Default::default(),
        }
    }

    #[tokio::test]
    async fn estimate_sell() {
        let mut zeroex_api = MockZeroExApi::new();

        // Response was generated with:
        //
        // curl "https://api.0x.org/swap/v1/price?\
        //     sellToken=0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2&\
        //     buyToken=0x6810e776880c02933d47db1b9fc05908e5386b96&\
        //     slippagePercentage=0&\
        //     sellAmount=100000000000000000"
        zeroex_api.expect_get_swap().return_once(|_| {
            Ok(SwapResponse {
                price: PriceResponse {
                    sell_amount: 100000000000000000u64.into(),
                    buy_amount: 1110165823572443613u64.into(),
                    allowance_target: addr!("def1c0ded9bec7f1a1670819833240f027b25eff"),
                    price: 11.101_658_235_724_436,
                    estimated_gas: 111000,
                },
                to: addr!("def1c0ded9bec7f1a1670819833240f027b25eff"),
                value: 42.into(),
                data: vec![1, 2, 3, 4],
            })
        });

        let weth = testlib::tokens::WETH;
        let gno = testlib::tokens::GNO;

        let trader = create_trader(Arc::new(zeroex_api));

        let trade = trader
            .get_trade(&Query {
                sell_token: weth,
                buy_token: gno,
                in_amount: 100000000000000000u64.into(),
                kind: OrderKind::Sell,
            })
            .await
            .unwrap();

        assert_eq!(trade.out_amount, 1110165823572443613u64.into());
        assert!(trade.gas_estimate > 111000);
        assert_eq!(
            trade.approval_spender,
            Some(addr!("def1c0ded9bec7f1a1670819833240f027b25eff")),
        );
        assert_eq!(
            trade.interaction,
            Interaction {
                target: addr!("def1c0ded9bec7f1a1670819833240f027b25eff"),
                value: 42.into(),
                data: vec![1, 2, 3, 4],
            }
        );
    }

    #[tokio::test]
    async fn estimate_buy() {
        let mut zeroex_api = MockZeroExApi::new();

        // Response was generated with:
        //
        // curl "https://api.0x.org/swap/v1/price?\
        //     sellToken=0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2&\
        //     buyToken=0x6810e776880c02933d47db1b9fc05908e5386b96&\
        //     slippagePercentage=0&\
        //     buyAmount=100000000000000000"
        zeroex_api.expect_get_swap().return_once(|_| {
            Ok(SwapResponse {
                price: PriceResponse {
                    sell_amount: 8986186353137488u64.into(),
                    buy_amount: 100000000000000000u64.into(),
                    allowance_target: addr!("def1c0ded9bec7f1a1670819833240f027b25eff"),
                    price: 0.089_861_863_531_374_87,
                    estimated_gas: 111000,
                },
                data: vec![5, 6, 7, 8],
                ..Default::default()
            })
        });

        let weth = testlib::tokens::WETH;
        let gno = testlib::tokens::GNO;

        let trader = create_trader(Arc::new(zeroex_api));

        let trade = trader
            .get_trade(&Query {
                sell_token: weth,
                buy_token: gno,
                in_amount: 100000000000000000u64.into(),
                kind: OrderKind::Buy,
            })
            .await
            .unwrap();

        assert_eq!(trade.out_amount, 8986186353137488u64.into());
        assert!(trade.gas_estimate > 111000);
        assert_eq!(trade.interaction.data, [5, 6, 7, 8]);
    }

    #[tokio::test]
    #[ignore]
    async fn real_estimate() {
        let weth = testlib::tokens::WETH;
        let gno = testlib::tokens::GNO;

        let zeroex_api = DefaultZeroExApi::with_default_url(Client::new());
        let trader = create_trader(Arc::new(zeroex_api));

        let trade = trader
            .get_trade(&Query {
                sell_token: weth,
                buy_token: gno,
                in_amount: 10u128.pow(18).into(),
                kind: OrderKind::Sell,
            })
            .await
            .unwrap();

        let gno = trade.out_amount.to_f64_lossy() / 1e18;
        println!("1.0 ETH buys {gno} GNO");
        println!("gas:  {}", trade.gas_estimate);
        println!("data: 0x{}", hex::encode(&trade.interaction.data));
    }
}