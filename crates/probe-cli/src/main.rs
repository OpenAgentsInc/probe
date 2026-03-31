use probe_core::backend_profiles::psionic_qwen35_2b_q8_registry;
use probe_core::runtime_bootstrap;
use probe_provider_openai::OpenAiProviderConfig;

fn main() {
    let bootstrap = runtime_bootstrap();
    let provider = OpenAiProviderConfig::from_backend_profile(&psionic_qwen35_2b_q8_registry());
    println!(
        "probe bootstrap complete: protocol=v{} base_url={} model={}",
        bootstrap.protocol.version, provider.base_url, provider.model
    );
}
