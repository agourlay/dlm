use crate::{random_user_agent, UserAgent};
use reqwest::redirect::Policy;
use reqwest::{Client, Proxy};
use std::time::Duration;

pub fn make_client(
    user_agent: &Option<UserAgent>,
    proxy: &Option<String>,
    redirect: bool,
) -> reqwest::Result<Client> {
    let client_builder = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(0);

    // setup user-agent
    let client_builder = match user_agent {
        Some(UserAgent::CustomUserAgent(ua)) => client_builder.user_agent(ua),
        Some(UserAgent::RandomUserAgent) => client_builder.user_agent(random_user_agent()),
        _ => client_builder,
    };

    // setup proxy
    let client_builder = match proxy {
        Some(p) => client_builder.proxy(Proxy::all(p)?),
        _ => client_builder,
    };

    // setup redirect
    let client_builder = if redirect {
        // defaults to 10 redirects
        client_builder.redirect(Policy::default())
    } else {
        client_builder.redirect(Policy::none())
    };

    client_builder.build()
}
