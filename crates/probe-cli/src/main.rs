use probe_core::runtime_bootstrap;
use probe_provider_openai::OpenAiProviderConfig;

fn main() {
    let bootstrap = runtime_bootstrap();
    let provider = OpenAiProviderConfig::localhost("unset");
    println!(
        "probe bootstrap complete: protocol=v{} base_url={}",
        bootstrap.protocol.version, provider.base_url
    );
}
