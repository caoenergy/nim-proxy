"use strict";

const LOGIN_ERRORS = Object.freeze({
  invalid_credentials: "Incorrect username or password.",
});
const loginError = LOGIN_ERRORS[document.body.dataset.errorCode];
if (loginError) {
  const node = document.getElementById("login-error");
  node.textContent = loginError;
  node.hidden = false;
}
