我想做一个语音输入法 MoliWhisper，但是我的思路比较特殊：
1. 通过某种方式调用 doubao 的 web 版本：https://www.doubao.com/chat（前提用户登录了）中的语音输入法模块
2. 读取 input 中 doubao 处理好了文本
3. 返回给用户

我想要的交互是一个 原生的 mac app，交互方式是：
* Press Right ⌥ Option (Mac) start / stop recording
* 开始语音识别时，会在当前屏幕顶层展示一个小框框，提示用户已经在语音识别了
* 小框框中会实时展示语音识别到的文本
* Press ESC at any stage to immediately cancel — nothing is transcribed or copied
* 语音识别结束后，把文本复制到粘贴板，如果当前用户光标在一个输入框上，则自动粘贴进去。小框框也会消失

我的方案是可行的！

我目前所知道的信息：

## 如何判断是否已登录：

1. 打开浏览器，打开url : https://www.doubao.com/chat
2. 如果网络请求中 doubao-user-api.md 所描述的 api 接口正常返回数据，则说明用户已经登录了
3. 反之如果没有调用那个接口，并且如果页面中存在元素 button[data-testid="to_login_button"]，则点击这个 login button 后会弹出登录窗口，用户登录完成后，会自动重定到 https://www.doubao.com/chat/?from_login=1，并且会成功调用 2 所描述的 api

## 登录后的操作
### 如何开始语音输入
4. 登录就绪后，在 /chat 页面会存在一个触发语音输入的按钮 asr_btn: data-testid="asr_btn"，默认状态是：data-state="inactive"
5. 点击 asr_btn 后，按钮会激活为 data-state="active"
6. 然后浏览器会提示需要麦克风权限，给予权限后，页面会一直通过麦克风拾音，并发起 doubao-wss.md 所描述的 wss api
### 如何结束语音输入
6. 用户觉得说完了后，需要手动再次点击 asr_btn 结束本次语音输入
7. 然后豆包会自动将用户语音输入的文字通过 POST https://www.doubao.com/chat/completion 发给大模型（你需要 block 这个接口，因为我们不需要，我们只需要语音输入的文本）

## 容易出现异常的场景
语音输入结束后，doubao 会尝试将结果发给大模型，虽然我们已经 block 了请求，但是前端 UI 中的语音输入按钮还是会消失几秒，变成一个 data-testid="chat_input_local_break_button" 的打断按钮，直到 api 请求超时。所以此时我们需要及时的触发这个打断按钮，确保下一次语音输入正常。

另外我发现在点击 asr_btn 后，会尝试建立 wss 连接，在建立连接之前此按钮会展示为 loading 态。此时按钮内部的元素为一个 data-dbx-name="spinner-root" 的 loading 态。此时 wss 连接也没有连接上呢。所以此时 app 的 overlay 应该也展示一个 loading 态，实时的与 webview 中的状态联动，其实我觉得最保险的判断方式还是通过 wss 连接的监听比较靠谱。
应该从发起 wss 开始


你需要给出你的技术实现方案。