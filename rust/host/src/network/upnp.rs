//! Bound router-controlled XML before any recursive tree parsing.
use anyhow::{Context, Result, bail};
use igd_next::{
    RequestError, SearchOptions,
    aio::{Gateway, Provider},
};
use reqwest::{Client, Url};
use std::{collections::HashMap, net::SocketAddr, time::Duration};
use xml::reader::{EventReader, XmlEvent};
use xmltree::Element;

const LIMIT: usize = 256 * 1024;

fn check_xml(bytes: &[u8]) -> Result<()> {
    if bytes.len() > LIMIT {
        bail!("UPnP response exceeds its limit");
    }
    let mut depth = 0;
    for event in EventReader::new(bytes) {
        match event? {
            XmlEvent::StartElement { .. } => {
                depth += 1;
                if depth > 64 {
                    bail!("UPnP XML exceeds its nesting limit");
                }
            }
            XmlEvent::EndElement { .. } => depth -= 1,
            _ => {}
        }
    }
    Ok(())
}
pub(super) fn xml(bytes: &[u8]) -> Result<Element> {
    check_xml(bytes)?;
    Ok(Element::parse(bytes)?)
}
pub(super) async fn bytes(mut response: reqwest::Response) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if result.len() + chunk.len() > LIMIT {
            bail!("UPnP response exceeds its limit");
        }
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}
fn client() -> Result<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()?)
}
pub(super) struct Bounded;
impl Provider for Bounded {
    async fn send_async(url: &str, action: &str, body: &str) -> Result<String, RequestError> {
        let request = async {
            let response = client()?
                .post(url)
                .header("SOAPAction", action)
                .header("Content-Type", "text/xml")
                .body(body.to_owned())
                .send()
                .await?;
            let bytes = bytes(response).await?;
            check_xml(&bytes)?;
            Ok::<_, anyhow::Error>(String::from_utf8(bytes)?)
        };
        request
            .await
            .map_err(|e| RequestError::IoError(std::io::Error::other(e.to_string())))
    }
}
fn text(node: &Element, name: &str) -> Option<String> {
    node.get_child(name)?.get_text().map(|s| s.into_owned())
}
fn service(node: &Element) -> Option<&Element> {
    if node.name == "service"
        && text(node, "serviceType").is_some_and(|value| {
            matches!(
                value.as_str(),
                "urn:schemas-upnp-org:service:WANIPConnection:1"
                    | "urn:schemas-upnp-org:service:WANIPConnection:2"
                    | "urn:schemas-upnp-org:service:WANPPPConnection:1"
            )
        })
    {
        return Some(node);
    }
    node.children
        .iter()
        .filter_map(xmltree::XMLNode::as_element)
        .find_map(service)
}
fn endpoint(base: &Url, value: &str, location: &Url) -> Result<Url> {
    let url = base.join(value.trim())?;
    if url.scheme() != "http"
        || url.host_str() != location.host_str()
        || url.port_or_known_default() != location.port_or_known_default()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("UPnP URL does not belong to the discovered gateway");
    }
    Ok(url)
}
async fn from_location(
    client: &Client,
    location: Url,
    peer: SocketAddr,
) -> Result<Gateway<Bounded>> {
    if location.scheme() != "http"
        || location.host_str().and_then(|h| h.parse().ok()) != Some(peer.ip())
        || !location.username().is_empty()
        || location.password().is_some()
    {
        bail!("UPnP descriptor does not belong to the responding gateway");
    }
    let root = xml(&bytes(
        client
            .get(location.clone())
            .send()
            .await?
            .error_for_status()?,
    )
    .await?)?;
    let service = service(&root).context("UPnP gateway has no WAN service")?;
    let base = match text(&root, "URLBase").filter(|base| !base.trim().is_empty()) {
        Some(base) => endpoint(&location, &base, &location)?,
        None => location.clone(),
    };
    let control = endpoint(
        &base,
        &text(service, "controlURL").context("UPnP control URL missing")?,
        &location,
    )?;
    let schema = endpoint(
        &base,
        &text(service, "SCPDURL").context("UPnP schema URL missing")?,
        &location,
    )?;
    let root = xml(&bytes(
        client
            .get(schema.clone())
            .send()
            .await?
            .error_for_status()?,
    )
    .await?)?;
    let actions = root
        .get_child("actionList")
        .context("UPnP schema has no actions")?;
    let mut control_schema = HashMap::new();
    for action in actions
        .children
        .iter()
        .filter_map(xmltree::XMLNode::as_element)
    {
        let Some(name) = text(action, "name") else {
            continue;
        };
        let arguments = action
            .get_child("argumentList")
            .map(|list| {
                list.children
                    .iter()
                    .filter_map(xmltree::XMLNode::as_element)
                    .filter(|arg| text(arg, "direction").as_deref() == Some("in"))
                    .filter_map(|arg| text(arg, "name"))
                    .collect()
            })
            .unwrap_or_default();
        control_schema.insert(name, arguments);
    }
    Ok(Gateway {
        addr: SocketAddr::new(
            peer.ip(),
            location
                .port_or_known_default()
                .context("UPnP port missing")?,
        ),
        root_url: location[url::Position::BeforePath..].to_owned(),
        control_url: control[url::Position::BeforePath..].to_owned(),
        control_schema_url: schema[url::Position::BeforePath..].to_owned(),
        control_schema,
        provider: Bounded,
    })
}
pub(super) async fn discover(options: SearchOptions) -> Result<Gateway<Bounded>> {
    tokio::time::timeout(options.timeout.unwrap_or(Duration::from_secs(3)), async {
        let socket = tokio::net::UdpSocket::bind(options.bind_addr).await?;
        socket.send_to(b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n", options.broadcast_address).await?;
        let client = client()?;
        let mut response = [0; 4096];
        for _ in 0..16 {
            let (count, peer) = socket.recv_from(&mut response).await?;
            let location = std::str::from_utf8(&response[..count]).ok().and_then(|text| {
                text.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("location").then(|| Url::parse(value.trim()).ok()).flatten()
                })
            });
            if let Some(location) = location
                && let Ok(gateway) = from_location(&client, location, peer).await {
                return Ok(gateway);
            }
        }
        bail!("no IPv4 gateway discovered")
    }).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn router_xml_is_bounded_before_building_a_tree() {
        assert!(xml(b"<root><device /></root>").is_ok());
        let nested = format!("{}{}", "<a>".repeat(5000), "</a>".repeat(5000));
        assert!(
            xml(nested.as_bytes())
                .unwrap_err()
                .to_string()
                .contains("nesting limit")
        );
        assert!(xml(&vec![b' '; LIMIT + 1]).is_err());
        assert!(xml(b"<root>").is_err());
    }
    #[tokio::test]
    async fn descriptor_and_schema_use_the_bounded_path() {
        use axum::{Router, routing::get};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/root.xml", get(|| async { "<root><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>/control</controlURL><SCPDURL>/schema.xml</SCPDURL></service></serviceList></device></root>" }))
            .route("/schema.xml", get(|| async { "<scpd><actionList><action><name>AddPortMapping</name><argumentList><argument><name>NewExternalPort</name><direction>in</direction></argument></argumentList></action></actionList></scpd>" }))
            .route("/deep.xml", get(|| async { format!("{}{}", "<a>".repeat(5000), "</a>".repeat(5000)) }));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = client().unwrap();
        let gateway = from_location(
            &client,
            Url::parse(&format!("http://{addr}/root.xml")).unwrap(),
            addr,
        )
        .await
        .unwrap();
        assert_eq!(gateway.control_url, "/control");
        assert_eq!(
            gateway.control_schema["AddPortMapping"],
            ["NewExternalPort"]
        );
        assert!(
            from_location(
                &client,
                Url::parse(&format!("http://{addr}/deep.xml")).unwrap(),
                addr
            )
            .await
            .is_err()
        );
        server.abort();
    }
}
