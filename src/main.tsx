/* @refresh reload */
import { render } from "solid-js/web";
import { App } from "./App";
import "./styles.css";
import { initTheme } from "./theme";
import { initI18n } from "./i18n";

initI18n();
initTheme();
render(() => <App />, document.getElementById("root") as HTMLElement);
