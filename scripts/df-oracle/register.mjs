
import { registerHooks } from 'node:module';
const parentURL = process.env.DF_ORACLE_PACKAGE_URL;
const macros = {
 '@lingui/core/macro': new URL('./scripts/test-unit/linguiCoreMacroStub.mjs', import.meta.url).href,
 '@lingui/react/macro': new URL('./scripts/test-unit/linguiReactMacroStub.mjs', import.meta.url).href,
};
registerHooks({
 resolve(specifier, context, next) {
  if (macros[specifier]) return { url: macros[specifier], shortCircuit:true };
  try {return next(specifier,context);} catch(error) {
   if (!specifier.startsWith('.') && !specifier.startsWith('/') && !specifier.includes(':'))
    return next(specifier,{...context,parentURL});
   throw error;
  }
 },
});
