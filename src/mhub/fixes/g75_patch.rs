pub(super) const MODULE: &str = "assets/main-DIt5vAcV.js";
pub(super) const STAMP: &str = "/* Peripheral Desk G75 keymap v2 */";

// Match the complete set of call sites before making any changes. If the user
// already rewrote this component, keep their implementation intact.
const HOOKS: &[(&str, &str)] = &[
    (
        "const Ia=ref([]),$a=ref(0);let Ra=0;",
        concat!(
            include_str!("g75-keymap.js"),
            "\nconst Ia=ref([]),$a=ref(0);let Ra=0;",
            "const pdG75=ma==='G75_Pro'&&!xa.value?createG75Editor(ha,keyList$6,[...baseArr,...mouseArr,...mediaArr,...extendedArr],va):null;",
            "function pdSave(index,preparation){const handler=navigator.deviceHandler,layer=da.keyLayer;",
            "return pdG75.save(handler,layer,Ia.value[index],preparation).catch(error=>{",
            "messageError({message:error.message});",
            "if(pa&&handler===navigator.deviceHandler&&layer===da.keyLayer)return Ua({localLoading:true})})}",
        ),
    ),
    (
        "const La=()=>{Ia.value=JSON.parse(JSON.stringify(ha)),syncKeyLabels",
        "const La=()=>{if(pdG75)return Ua({localLoading:true});Ia.value=JSON.parse(JSON.stringify(ha)),syncKeyLabels",
    ),
    (
        "const Ua=({localLoading:Ha=!1}={})=>{const Ya=++Ra;Ia.value=JSON.parse(JSON.stringify(ha));",
        "const Ua=async({localLoading:Ha=!1}={})=>{if(pdG75)await pdG75.waitForWrites();const Ya=++Ra;Ia.value=pdG75?(pdG75.beginRead(),[]):JSON.parse(JSON.stringify(ha));",
    ),
    (
        "wiredParser:_r,wirelessParser:Sr});const Dr=Ha?null:ca();",
        concat!(
            "wiredParser:pdG75?pdG75.withTail(_r):_r,",
            "wirelessParser:pdG75?pdG75.withTail(Sr):Sr});const Dr=Ha?null:ca();",
        ),
    ),
    (
        r#"return mr.sendCommand("get","getKeySetting",{}).then(Lr=>{!pa||Ya!==Ra||"#,
        concat!(
            "const pdRead=()=>mr.sendCommand('get','getKeySetting',{});",
            "return (pdG75?pdG75.waitForWrites().then(pdRead):pdRead()).then(Lr=>{",
            "if(pdG75){if(pa&&Ya===Ra&&mr===navigator.deviceHandler){",
            "Ia.value=pdG75.receive(mr,qa,Qa,Lr);Ir()}return}",
            "!pa||Ya!==Ra||",
        ),
    ),
    (
        r#"Ua(),watch(()=>da.keyboardTab,Ha=>{xa.value&&Ha==="lightSetting"&&Ua()});"#,
        concat!(
            r#"if(ma!=="G75_Pro"||!xa.value)Ua();"#,
            r#"watch(()=>da.keyboardTab,Ha=>{if(ma==="G75_Pro"){if(xa.value===(Ha==="lightSetting"))Ua()}else if(xa.value&&Ha==="lightSetting")Ua()});"#,
        ),
    ),
    (
        "syncKeyLabels(Ta.value,Ia.value)}).finally(()=>{Ha?$a.value-=1:Dr()})",
        concat!(
            "syncKeyLabels(Ta.value,Ia.value)}).catch(error=>{if(!pdG75)throw error;",
            "if(pa&&Ya===Ra)messageError({message:error.message})}).finally(()=>{Ha?$a.value-=1:Dr()})",
        ),
    ),
    (
        "try{await lr,await za()}catch(yr){Ia.value[Qa]=ar",
        "try{if(pdG75)await pdSave(Qa,lr);else{await lr;await za()}}catch(yr){Ia.value[Qa]=ar",
    ),
    (
        "Ia.value[_r].fn2Val=Qa);za()}},gr=async",
        "Ia.value[_r].fn2Val=Qa);await (pdG75?pdSave(_r):za())}},gr=async",
    ),
    (
        "Fa=Ha=>{Ia.value[Ha]=deepCopy$1(ha[Ha]),za()}",
        "Fa=Ha=>{Ia.value[Ha]=deepCopy$1(ha[Ha]);return pdG75?pdSave(Ha):za()}",
    ),
];

pub(super) fn patched(source: &str) -> Option<String> {
    let mut edits = Vec::with_capacity(HOOKS.len());
    for &(needle, replacement) in HOOKS {
        let mut hits = source.match_indices(needle);
        let (offset, _) = hits.next()?;
        if hits.next().is_some() {
            return None;
        }
        edits.push((offset, needle.len(), replacement));
    }
    edits.sort_unstable_by_key(|edit| edit.0);
    let mut out = String::with_capacity(source.len() + 8192);
    out.push_str(STAMP);
    let mut last = 0;
    for (offset, length, replacement) in edits {
        out.push_str(&source[last..offset]);
        out.push_str(replacement);
        last = offset + length;
    }
    out.push_str(&source[last..]);
    Some(out)
}
