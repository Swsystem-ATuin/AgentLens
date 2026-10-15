// All credential operations stay on this configured same-origin application.
(() => {
  "use strict";
  const decode = (v) => Uint8Array.from(atob(v.replace(/-/g, "+").replace(/_/g, "/")), c => c.charCodeAt(0));
  const encode = (v) => btoa(String.fromCharCode(...new Uint8Array(v))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  const post = async (path, value) => {
    const response = await fetch(path, {method: "POST", credentials: "same-origin", headers: {"Content-Type": "application/json"}, body: JSON.stringify(value)});
    if (!response.ok) throw new Error("Passkey action rejected. Check password, factor, configuration, or challenge expiry.");
    return response.json();
  };
  const fields = (form) => Object.fromEntries(new FormData(form));
  const message = (form, value) => {
    let status = form.querySelector("[data-passkey-status]") || document.getElementById("passkey-status");
    if (!status) {status = document.createElement("p"); status.dataset.passkeyStatus = "1"; status.setAttribute("role", "status"); form.append(status);}
    status.textContent = value;
  };
  const credential = (c) => ({id: c.id, rawId: encode(c.rawId), type: c.type, extensions: c.getClientExtensionResults(), response: {
    clientDataJSON: encode(c.response.clientDataJSON),
    ...(c.response.attestationObject ? {attestationObject: encode(c.response.attestationObject), transports: c.response.getTransports ? c.response.getTransports() : []} : {authenticatorData: encode(c.response.authenticatorData), signature: encode(c.response.signature), userHandle: c.response.userHandle ? encode(c.response.userHandle) : null})
  }});
  const ceremony = async (form, mode) => {
    if (!window.PublicKeyCredential || !window.isSecureContext) throw new Error("A secure browser context and compatible authenticator are required.");
    const input = fields(form);
    const base = mode === "login" ? "/login/passkey" : mode === "register" ? "/account/passkeys/register" : "/account/passkeys/auth";
    const start = await post(`${base}/start`, input);
    const options = start.options.publicKey;
    options.challenge = decode(options.challenge);
    if (options.user) options.user.id = decode(options.user.id);
    for (const key of [...(options.allowCredentials || []), ...(options.excludeCredentials || [])]) key.id = decode(key.id);
    const result = mode === "register" ? await navigator.credentials.create({publicKey: options}) : await navigator.credentials.get({publicKey: options});
    return post(`${base}/finish`, {csrf_token: input.csrf_token, ticket: start.ticket, response: credential(result)});
  };
  document.querySelectorAll("form").forEach((form) => {
    if (form.dataset.passkeyRegister !== undefined || form.dataset.passkeyDelete !== undefined) form.addEventListener("submit", async (event) => {
      event.preventDefault();
      try {
        const result = form.dataset.passkeyRegister !== undefined ? await ceremony(form, "register") : await post("/account/passkeys/delete", fields(form));
        if (result.redirect) {window.location.assign(result.redirect); return;}
        message(form, "Passkey enrolled. Save any recovery codes below now; they are shown only once. Reload before another device change.");
        document.getElementById("passkey-recovery").textContent = (result.codes || []).join("\n");
      } catch (error) {message(form, error.message);}
    });
    // Add a factor button to existing password-confirmed account/admin forms.
    if (form.querySelector('[name="current_password"]') && form.querySelector('[name="mfa_code"]') && !form.querySelector("[data-passkey-auth]")) {
      form.elements.mfa_code.maxLength = 128;
      const button = document.createElement("button"); button.type = "button"; button.dataset.passkeyAuth = "1"; button.textContent = "Verify a passkey second factor"; form.append(button);
    }
    form.querySelectorAll("[data-passkey-auth], [data-passkey-login]").forEach((button) => button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        const result = await ceremony(form, button.dataset.passkeyLogin !== undefined ? "login" : "auth");
        if (result.redirect) window.location.assign(result.redirect);
        else {form.elements.mfa_code.value = result.proof; message(form, "Passkey verified. Submit this action within 90 seconds; the proof works once.");}
      } catch (error) {message(form, error.message);} finally {button.disabled = false;}
    }));
  });
})();
