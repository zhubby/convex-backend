use std::{
    collections::BTreeMap,
    time::Duration,
};

use common::{
    runtime::{
        Runtime,
        UnixTimestamp,
    },
    types::EnvVarValue,
    value::{
        NamespacedTableMapping,
        TableMappingValue,
    },
};
use deno_core::v8;
use futures::{
    future,
    FutureExt,
};
use isolate::{
    environment::{
        AsyncOpRequest,
        IsolateEnvironment,
    },
    ConcurrencyPermit,
    Timeout,
};
use model::modules::module_versions::FullModuleSource;
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use runtime::testing::TestRuntime;
use serde_json::Value as JsonValue;
use tokio::task::JoinSet;

pub struct TestEnvironment {
    rt: TestRuntime,
    rng: ChaCha12Rng,

    next_timer_id: usize,
    timers: JoinSet<usize>,
    timer_resolvers: BTreeMap<usize, v8::Global<v8::PromiseResolver>>,
}

impl TestEnvironment {
    pub fn new(rt: TestRuntime) -> Self {
        Self {
            rt,
            rng: ChaCha12Rng::from_seed([0; 32]),

            next_timer_id: 0,
            timers: JoinSet::new(),
            timer_resolvers: BTreeMap::new(),
        }
    }
}

impl IsolateEnvironment<TestRuntime> for TestEnvironment {
    async fn lookup_source(
        &mut self,
        path: &str,
        _timeout: &mut Timeout<TestRuntime>,
        _permit: &mut Option<ConcurrencyPermit>,
    ) -> anyhow::Result<Option<FullModuleSource>> {
        if path != "test.js" {
            return Ok(None);
        }
        // NB: These files are generated by the *isolate* crate's build script.
        let source = include_str!("../../../../../npm-packages/simulation/dist/main.js");
        let source_map = include_str!("../../../../../npm-packages/simulation/dist/main.js.map");
        Ok(Some(FullModuleSource {
            source: source.to_string(),
            source_map: Some(source_map.to_string()),
        }))
    }

    fn syscall(&mut self, _name: &str, _args: JsonValue) -> anyhow::Result<JsonValue> {
        panic!("syscall() unimplemented");
    }

    fn start_async_syscall(
        &mut self,
        name: String,
        args: JsonValue,
        _resolver: v8::Global<v8::PromiseResolver>,
    ) -> anyhow::Result<()> {
        tracing::info!("Ignoring async syscall: {name:?} {args:?}");
        Ok(())
    }

    fn trace(
        &mut self,
        level: common::log_lines::LogLevel,
        messages: Vec<String>,
    ) -> anyhow::Result<()> {
        for message in messages {
            tracing::info!("[{level:?}] {message}");
        }
        Ok(())
    }

    fn rng(&mut self) -> anyhow::Result<&mut ChaCha12Rng> {
        Ok(&mut self.rng)
    }

    fn unix_timestamp(&self) -> anyhow::Result<UnixTimestamp> {
        Ok(self.rt.unix_timestamp())
    }

    fn get_environment_variable(
        &mut self,
        _name: common::types::EnvVarName,
    ) -> anyhow::Result<Option<EnvVarValue>> {
        Ok(None)
    }

    fn get_table_mapping_without_system_tables(&mut self) -> anyhow::Result<TableMappingValue> {
        panic!("get_table_mapping_without_system_tables() unimplemented");
    }

    fn get_all_table_mappings(&mut self) -> anyhow::Result<NamespacedTableMapping> {
        panic!("get_all_table_mappings() unimplemented");
    }

    fn start_async_op(
        &mut self,
        request: AsyncOpRequest,
        resolver: v8::Global<v8::PromiseResolver>,
    ) -> anyhow::Result<()> {
        match request {
            AsyncOpRequest::Sleep { until, .. } => {
                let id = self.next_timer_id;
                self.next_timer_id += 1;

                let now = self.rt.unix_timestamp();
                let duration = if until > now {
                    until - now
                } else {
                    Duration::ZERO
                };
                self.timers
                    .spawn(tokio::time::sleep(duration).map(move |_| id));
                self.timer_resolvers.insert(id, resolver);
            },
            req => {
                tracing::debug!("Ignoring async op request: {req:?}");
            },
        }
        Ok(())
    }

    fn user_timeout(&self) -> Duration {
        Duration::from_secs(60 * 60 * 24)
    }

    fn system_timeout(&self) -> Duration {
        Duration::from_secs(60 * 60 * 24)
    }
}

impl TestEnvironment {
    pub async fn next_timer(&mut self) -> anyhow::Result<v8::Global<v8::PromiseResolver>> {
        let Some(timer) = self.timers.join_next().await else {
            return future::pending().await;
        };
        let timer_id = timer?;
        let resolver = self
            .timer_resolvers
            .remove(&timer_id)
            .ok_or_else(|| anyhow::anyhow!("Timer resolver not found"))?;
        Ok(resolver)
    }
}
