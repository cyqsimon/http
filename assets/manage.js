window.addEventListener("load", function() {
  let new_directory_line = document.getElementById("new_directory");
  let delete_file_links = document.getElementsByClassName("delete_file_icon");
  let rename_links = document.getElementsByClassName("rename_icon");


  if(new_directory_line) {
    let new_directory_filename_cell = new_directory_line.children[1];
    let new_directory_status_output = new_directory_line.children[4].children[0];
    let new_directory_filename_input = null;

    new_directory_line.addEventListener("click", function(ev) {
      if(new_directory_filename_input === null)
        ev.preventDefault();
      else if(ev.target === new_directory_status_output)
        ;
      else if(ev.target !== new_directory_filename_input) {
        ev.preventDefault();
        new_directory_filename_input.focus();
      }

      if(new_directory_filename_input === null) {
        let submit_callback = function() {
          create_new_directory(new_directory_filename_input.value, new_directory_status_output);
        };

        new_directory_filename_input = make_filename_input(new_directory_filename_cell, "", submit_callback);
        make_confirm_icon(new_directory_status_output, submit_callback);
      }
    }, true);
  }

  for(let i = delete_file_links.length - 1; i >= 0; --i) {
    let link = delete_file_links[i];

    link.addEventListener("click", function(ev) {
      ev.preventDefault();

      let line = link.parentElement.parentElement;
      make_request("DELETE", line.children[0].children[0].href, link);
    });
  }

  let first_rename_onclick = function(link, first_onclick, ev) {
    ev.preventDefault();

    let line = link.parentElement.parentElement;
    let filename_cell = line.children[1];
    let original_name = filename_cell.innerText;

    let submit_callback = function() {
      rename(original_name, new_name_input.value, link);
    };
    let new_name_input = make_filename_input(filename_cell, original_name, submit_callback);

    link.removeEventListener("click", first_onclick);
    make_confirm_icon(link, submit_callback);
  };
  for(let i = rename_links.length - 1; i >= 0; --i) {
    let link = rename_links[i];

    let first_onclick = function(ev) {
      first_rename_onclick(link, first_onclick, ev);
    };
    link.addEventListener("click", first_onclick);
  }


  function make_filename_input(input_container, initial, callback) {
    input_container.innerHTML = "<input type=\"text\"></input>";
    let input_elem = input_container.children[0];
    input_elem.value = initial;

    input_elem.addEventListener("keypress", function(ev) {
      if(ev.keyCode === 13)  // Enter
        callback();
    });

    input_elem.focus();

    return input_elem;
  };

  function make_confirm_icon(element, callback) {
    element.classList.add("confirm_icon");
    element.href = "#confirm";
    element.innerText = "Confirm";
    element.addEventListener("click", callback);
  };

  function create_new_directory(fname, status_out) {
    let req_url = window.location.origin + window.location.pathname;
    if(!req_url.endsWith("/"))
      req_url += "/";
    req_url += encodeURI(fname);

    make_request("MKCOL", req_url, status_out);
  };

  function rename(fname_from, fname_to, status_out) {
    let root_url = window.location.origin + window.location.pathname;
    if(!root_url.endsWith("/"))
      root_url += "/";

    if(fname_from.endsWith("/"))
      fname_from = fname_from.substr(0, fname_from.length - 1);
    if(fname_to.endsWith("/"))
      fname_to = fname_to.substr(0, fname_to.length - 1);

    make_request("MOVE", root_url + encodeURI(fname_from), status_out, function(request) {
      request.setRequestHeader("Destination", root_url + encodeURI(fname_to));
    });
  };

  function make_request(verb, url, status_out, request_modifier) {
    let request = new XMLHttpRequest();
    request.addEventListener("loadend", function() {
      if(request.status >= 200 && request.status < 300)
        window.location.reload();
      else {
        status_out.innerHTML = request.status + " " + request.statusText + (request.response ? " — " : "") + request.response;
        status_out.classList.add("has-log");
      }
    });
    request.open(verb, url);
    if(request_modifier)
      request_modifier(request);
    request.send();
  };
});
