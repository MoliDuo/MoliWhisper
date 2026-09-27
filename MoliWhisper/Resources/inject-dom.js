// inject-dom.js
// Injected at document end for DOM interaction helpers.
(function() {
    'use strict';

    window.__moliWhisper = {
        isLoginButtonPresent: function() {
            return !!document.querySelector('button[data-testid="to_login_button"]');
        }
    };
})();
