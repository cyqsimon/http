//! WebDAV handling is based heavily on
//! https://github.com/tylerwhall/hyperdav-server/blob/415f512ac030478593ad389a3267aeed7441d826/src/lib.rs


use self::super::super::util::{CommaList, Depth, file_time_modified, file_time_created, is_actually_file, is_descendant_of, html_response, file_length,
                               file_binary, ERROR_HTML};
use xml::{EmitterConfig as XmlEmitterConfig, ParserConfig as XmlParserConfig};
use xml::reader::{EventReader as XmlReader, XmlEvent as XmlREvent};
use xml::writer::{EventWriter as XmlWriter, XmlEvent as XmlWEvent};
use std::io::{self, ErrorKind as IoErrorKind, Write};
use iron::{status, IronResult, Response, Request};
use xml::writer::Error as XmlWError;
use mime_guess::guess_mime_type_opt;
use xml::name::{Name, OwnedName};
use self::super::HttpHandler;
use xml::common::XmlVersion;
use iron::mime::Mime;
use std::path::Path;
use std::fs;


/*
davfs2 mount:

[2019-09-17 18:30:09] Request {
    url: Url { generic_url: "http://192.168.1.109:8000/" }
    method: Extension("PROPFIND")
    remote_addr: V4(192.168.1.109:3373)
    local_addr: V4(0.0.0.0:8000)
}
Headers { User-Agent: davfs2/1.5.4 neon/0.30.2
, Connection: TE
, TE: trailers
, Host: 192.168.1.109:8000
, Depth: 1
, Content-Length: 257
, Content-Type: application/xml
, }
<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:">
    <prop>
        <resourcetype xmlns="DAV:"/>
        <getcontentlength xmlns="DAV:"/>
        <getetag xmlns="DAV:"/>
        <getlastmodified xmlns="DAV:"/>
        <executable xmlns="http://apache.org/dav/props/"/>
    </prop>
</propfind>


Headers { User-Agent: davfs2/1.5.4 neon/0.30.2
, Connection: TE
, TE: trailers
, Host: 192.168.1.109:8000
, Depth: 0
, Content-Length: 159
, Content-Type: application/xml
, }
Propfind "\\\\?\\P:\\Rust\\http" [OwnedName { local_name: "quota-available-bytes", namespace: Some("DAV:"), prefix: None },
                                  OwnedName { local_name: "quota-used-bytes", namespace: Some("DAV:"), prefix: None }] 0
*/



lazy_static! {
    static ref DEFAULT_XML_PARSER_CONFIG: XmlParserConfig = XmlParserConfig { trim_whitespace: true, ..Default::default() };
    static ref DEFAULT_XML_EMITTER_CONFIG: XmlEmitterConfig = XmlEmitterConfig { perform_indent: cfg!(debug_assertions), ..Default::default() };
}


impl HttpHandler {
    pub(super) fn handle_webdav_propfind(&self, req: &mut Request) -> IronResult<Response> {
        let (req_p, symlink, url_err) = self.parse_requested_path(req);

        if url_err {
            return self.handle_invalid_url(req, "<p>Percent-encoding decoded to invalid UTF-8.</p>");
        }

        if !req_p.exists() || (symlink && !self.follow_symlinks) ||
           (symlink && self.follow_symlinks && self.sandbox_symlinks && !is_descendant_of(&req_p, &self.hosted_directory.1)) {
            return self.handle_nonexistant(req, req_p);
        }


        let depth = req.headers.get::<Depth>().copied().unwrap_or(Depth::Zero);

        let props = match parse_propfind(req) {
            Ok(props) => props,
            Err(e) => {
                log!("{green}{}{reset} tried to {red}PROPFIND{reset} {yellow}{}{reset} with invalid XML",
                     req.remote_addr,
                     req_p.display());
                return self.handle_generated_response_encoding(req,
                                                               status::BadRequest,
                                                               html_response(ERROR_HTML, &["400 Bad Request", &format!("Invalid XML: {}", e), ""]));
            }
        };

        log!("{green}{}{reset} requested {red}PROPFIND{reset} of {} on {yellow}{}{reset} at depth {}",
             req.remote_addr,
             CommaList(props.iter().map(|p| &p.local_name)),
             req_p.display(),
             depth);

        match self.handle_webdav_propfind_write_output(req, req.url.as_ref().as_str().to_string(), &req_p, &props, depth)
            .expect("Couldn't write PROPFIND XML") {
            Ok(xml_resp) => Ok(Response::with((status::MultiStatus, xml_resp, "text/xml;charset=utf-8".parse::<Mime>().unwrap()))),
            Err(resp) => resp,
        }
    }

    fn handle_webdav_propfind_write_output(&self, req: &mut Request, url: String, path: &Path, props: &[OwnedName], depth: Depth)
                                           -> Result<Result<Vec<u8>, IronResult<Response>>, XmlWError> {
        let mut resp = vec![];

        let mut xml_out = XmlWriter::new_with_config(&mut resp, DEFAULT_XML_EMITTER_CONFIG.clone());
        xml_out.write(XmlWEvent::StartDocument {
                version: XmlVersion::Version10,
                encoding: Some("utf-8"),
                standalone: None,
            })?;
        xml_out.write(XmlWEvent::start_element("D:multistatus").ns("D", "DAV:"))?;

        handle_propfind_path(&mut xml_out, &url, &path, &props)?;

        if path.metadata().expect("Failed to get requested file metadata").is_dir() {
            if let Some(ir) = self.handle_webdav_propfind_path_recursive(req, &mut xml_out, url, &path, &props, depth)? {
                return Ok(Err(ir));
            }
        }

        xml_out.write(XmlWEvent::end_element())?;

        Ok(Ok(resp))
    }

    fn handle_webdav_propfind_path_recursive<W: Write>(&self, req: &mut Request, out: &mut XmlWriter<W>, root_url: String, root_path: &Path,
                                                       props: &[OwnedName], depth: Depth)
                                                       -> Result<Option<IronResult<Response>>, XmlWError> {
        if let Some(next_depth) = depth.lower() {
            for f in root_path.read_dir().expect("Failed to read requested directory").map(|p| p.expect("Failed to iterate over requested directory")) {
                let mut url = root_url.clone();
                if !url.ends_with('/') {
                    url.push('/');
                }
                url.push_str(f.file_name().to_str().expect("Filename not UTF-8"));

                let mut path = f.path();
                let mut symlink = false;
                while let Ok(newlink) = path.read_link() {
                    symlink = true;
                    path = newlink;
                }

                if !(!path.exists() || (symlink && !self.follow_symlinks) ||
                     (symlink && self.follow_symlinks && self.sandbox_symlinks && !is_descendant_of(&path, &self.hosted_directory.1))) {
                    handle_propfind_path(out, &url, &path, props)?;
                    self.handle_webdav_propfind_path_recursive(req, out, url, &path, props, next_depth)?;
                }
            }
        }

        Ok(None)
    }

    pub(super) fn handle_webdav_proppatch(&self, req: &mut Request) -> IronResult<Response> {
        log!("{:#?}", req);
        eprintln!("{:?}", req.headers);
        io::copy(&mut req.body, &mut io::stderr()).unwrap();
        Ok(Response::with((status::MethodNotAllowed, "PROPPATCH unimplemented")))
    }

    pub(super) fn handle_webdav_mkcol(&self, req: &mut Request) -> IronResult<Response> {
        let (req_p, symlink, url_err) = self.parse_requested_path(req);

        log!("{green}{}{reset} requested to {red}MKCOL{reset} at {yellow}{}{reset}",
             req.remote_addr,
             req_p.display());

        if url_err {
            return self.handle_invalid_url(req, "<p>Percent-encoding decoded to invalid UTF-8.</p>");
        }

        if self.writes_temp_dir.is_none() {
            return self.handle_forbidden_method(req, "-w", "write requests");
        }

        if !req_p.parent().map(|pp| pp.exists()).unwrap_or(true) || (symlink && !self.follow_symlinks) ||
           (symlink && self.follow_symlinks && self.sandbox_symlinks && !is_descendant_of(&req_p, &self.hosted_directory.1)) {
            return self.handle_nonexistant(req, req_p);
        }

        match fs::create_dir(&req_p) {
            Ok(()) => Ok(Response::with(status::Created)),
            Err(e) => {
                match e.kind() {
                    IoErrorKind::NotFound => self.handle_nonexistant(req, req_p),
                    IoErrorKind::AlreadyExists => Ok(Response::with((status::MethodNotAllowed, "File exists"))),
                    _ => Ok(Response::with(status::Forbidden)),
                }
            }
        }
    }

    pub(super) fn handle_webdav_copy(&self, req: &mut Request) -> IronResult<Response> {
        log!("{:#?}", req);
        eprintln!("{:?}", req.headers);
        io::copy(&mut req.body, &mut io::stderr()).unwrap();
        Ok(Response::with((status::MethodNotAllowed, "COPY unimplemented")))
    }

    pub(super) fn handle_webdav_move(&self, req: &mut Request) -> IronResult<Response> {
        log!("{:#?}", req);
        eprintln!("{:?}", req.headers);
        io::copy(&mut req.body, &mut io::stderr()).unwrap();
        Ok(Response::with((status::MethodNotAllowed, "MOVE unimplemented")))
    }
}


fn parse_propfind(req: &mut Request) -> Result<Vec<OwnedName>, String> {
    #[derive(Debug, Copy, Clone, Hash, PartialOrd, Ord, PartialEq, Eq)]
    enum State {
        Start,
        PropFind,
        Prop,
        InProp,
    }


    let mut xml = XmlReader::new_with_config(&mut req.body, DEFAULT_XML_PARSER_CONFIG.clone());
    let mut state = State::Start;
    let mut props = vec![];

    loop {
        let event = xml.next().map_err(|e| e.to_string())?;

        match (state, event) {
            (State::Start, XmlREvent::StartDocument { .. }) => (),
            (State::Start, XmlREvent::StartElement { ref name, .. }) if name.local_name == "propfind" => state = State::PropFind,

            (State::PropFind, XmlREvent::StartElement { ref name, .. }) if name.local_name == "prop" => state = State::Prop,

            (State::Prop, XmlREvent::StartElement { name, .. }) => {
                state = State::InProp;
                props.push(name);
            }
            (State::Prop, XmlREvent::EndElement { .. }) => return Ok(props),

            (State::InProp, XmlREvent::EndElement { .. }) => state = State::Prop,

            (st, ev) => return Err(format!("Unexpected event {:?} during state {:?}", ev, st)),
        }
    }
}

fn handle_propfind_path<W: Write>(out: &mut XmlWriter<W>, url: &str, path: &Path, props: &[OwnedName]) -> Result<(), XmlWError> {
    out.write(XmlWEvent::start_element("D:response"))?;

    out.write(XmlWEvent::start_element("D:href"))?;
    out.write(XmlWEvent::characters(url))?;
    out.write(XmlWEvent::end_element())?; // href

    let mut failed_props = Vec::with_capacity(props.len());
    out.write(XmlWEvent::start_element("D:propstat"))?;
    out.write(XmlWEvent::start_element("D:prop"))?;
    for prop in props {
        if !handle_prop_path(out, path, prop.borrow())? {
            failed_props.push(prop);
        }
    }
    out.write(XmlWEvent::end_element())?; // prop
    out.write(XmlWEvent::start_element("D:status"))?;
    if failed_props.len() >= props.len() {
        // If they all failed, make this a failure response and return
        out.write(XmlWEvent::characters("HTTP/1.1 404 Not Found"))?;
        out.write(XmlWEvent::end_element())?; // status
        out.write(XmlWEvent::end_element())?; // propstat
        out.write(XmlWEvent::end_element())?; // response
        return Ok(());
    }
    out.write(XmlWEvent::characters("HTTP/1.1 200 OK"))?;
    out.write(XmlWEvent::end_element())?; // status
    out.write(XmlWEvent::end_element())?; // propstat

    // Handle the failed properties
    out.write(XmlWEvent::start_element("D:propstat"))?;
    out.write(XmlWEvent::start_element("D:prop"))?;
    for prop in failed_props {
        write_client_prop(out, prop.borrow())?;
        out.write(XmlWEvent::end_element())?;
    }
    out.write(XmlWEvent::end_element())?; // prop
    out.write(XmlWEvent::start_element("D:status"))?;
    out.write(XmlWEvent::characters("HTTP/1.1 404 Not Found"))?;
    out.write(XmlWEvent::end_element())?; // status
    out.write(XmlWEvent::end_element())?; // propstat
    out.write(XmlWEvent::end_element())?; // response
    Ok(())
}

fn handle_prop_path<W: Write>(out: &mut XmlWriter<W>, path: &Path, prop: Name) -> Result<bool, XmlWError> {
    match (prop.namespace, prop.local_name) {
        (Some("DAV:"), "resourcetype") => {
            out.write(XmlWEvent::start_element("D:resourcetype"))?;
            if !is_actually_file(&path.metadata().expect("Failed to get requested file metadata").file_type()) {
                out.write(XmlWEvent::start_element("D:collection"))?;
                out.write(XmlWEvent::end_element())?;
            }
        }

        (Some("DAV:"), "creationdate") => {
            out.write(XmlWEvent::start_element("D:creationdate"))?;
            out.write(XmlWEvent::characters(&file_time_created(&path).rfc3339().to_string()))?;
        }

        (Some("DAV:"), "getlastmodified") => {
            out.write(XmlWEvent::start_element("D:getlastmodified"))?;
            out.write(XmlWEvent::characters(&file_time_modified(&path).rfc3339().to_string()))?;
        }

        (Some("DAV:"), "getcontentlength") => {
            out.write(XmlWEvent::start_element("D:getcontentlength"))?;
            out.write(XmlWEvent::characters(&file_length(&path.metadata().expect("Failed to get requested file metadata"), &path).to_string()))?;
        }

        (Some("DAV:"), "getcontenttype") => {
            out.write(XmlWEvent::start_element("D:getcontenttype"))?;
            let mime_type = guess_mime_type_opt(&path).unwrap_or_else(|| if file_binary(&path) {
                "application/octet-stream".parse().unwrap()
            } else {
                "text/plain".parse().unwrap()
            });
            out.write(XmlWEvent::characters(&mime_type.to_string()))?;
        }

        _ => return Ok(false),
    }

    out.write(XmlWEvent::end_element())?;
    Ok(true)
}

fn write_client_prop<W: Write>(out: &mut XmlWriter<W>, prop: Name) -> Result<(), XmlWError> {
    if let Some(namespace) = prop.namespace {
        if let Some(prefix) = prop.prefix {
            // Remap the client's prefix if it overlaps with our DAV: prefix
            if prefix == "D" && namespace != "DAV:" {
                return out.write(XmlWEvent::start_element(Name {
                        local_name: prop.local_name,
                        namespace: Some(namespace),
                        prefix: Some("U"),
                    })
                    .ns("U", namespace));
            }
        }
    }
    out.write(XmlWEvent::start_element(prop))
}
