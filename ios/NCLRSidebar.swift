import SwiftUI
import UIKit

@objc(NCLRSidebarController)
final class NCLRSidebarController: UIViewController {
    private let state = NCLRSidebarState()
    private let palette: NCLRSidebarPalette
    private let selectionHandler: (Int) -> Void

    @objc(initWithCanvasColor:selectionColor:selectedForegroundColor:foregroundColor:accentColor:selectionHandler:)
    init(
        canvasColor: UIColor,
        selectionColor: UIColor,
        selectedForegroundColor: UIColor,
        foregroundColor: UIColor,
        accentColor: UIColor,
        selectionHandler: @escaping (Int) -> Void
    ) {
        palette = NCLRSidebarPalette(
            canvas: Color(uiColor: canvasColor),
            selection: Color(uiColor: selectionColor),
            selectedForeground: Color(uiColor: selectedForegroundColor),
            foreground: Color(uiColor: foregroundColor),
            accent: Color(uiColor: accentColor)
        )
        self.selectionHandler = selectionHandler
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("Use the sidebar initializer")
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
        let hosting = UIHostingController(rootView: NCLRSidebarView(
            state: state, palette: palette, selectionHandler: selectionHandler
        ))
        hosting.view.backgroundColor = .clear
        addChild(hosting)
        hosting.view.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(hosting.view)
        NSLayoutConstraint.activate([
            hosting.view.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            hosting.view.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            hosting.view.topAnchor.constraint(equalTo: view.topAnchor),
            hosting.view.bottomAnchor.constraint(equalTo: view.bottomAnchor)
        ])
        hosting.didMove(toParent: self)
    }

    @objc(setSelectedPage:animated:)
    func setSelectedPage(_ page: Int, animated: Bool) {
        state.animateSelection = animated && !UIAccessibility.isReduceMotionEnabled
        state.selectedPage = page
    }

    @objc(updateAccountWithName:detail:avatarURL:)
    func updateAccount(name: String, detail: String, avatarURL: String?) {
        let url = avatarURL.flatMap(URL.init(string:))
        let account = NCLRSidebarAccount(
            name: name, detail: detail,
            avatarURL: url?.scheme?.lowercased() == "https" ? url : nil
        )
        if state.account != account {
            state.account = account
        }
    }
}

private struct NCLRSidebarPalette {
    let canvas: Color
    let selection: Color
    let selectedForeground: Color
    let foreground: Color
    let accent: Color
}

private struct NCLRSidebarAccount: Equatable {
    var name = "未登录"
    var detail = "Microsoft 账号"
    var avatarURL: URL?
}

private final class NCLRSidebarState: ObservableObject {
    @Published var selectedPage = 0
    @Published var account = NCLRSidebarAccount()
    var animateSelection = false
}

private struct NCLRSidebarRowBounds: PreferenceKey {
    static var defaultValue: [Int: Anchor<CGRect>] { [:] }

    static func reduce(value: inout [Int: Anchor<CGRect>],
                       nextValue: () -> [Int: Anchor<CGRect>]) {
        value.merge(nextValue(), uniquingKeysWith: { _, latest in latest })
    }
}

private struct NCLRSidebarView: View {
    @ObservedObject var state: NCLRSidebarState
    let palette: NCLRSidebarPalette
    let selectionHandler: (Int) -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let items = [
        (title: "主页", symbol: "house.fill"),
        (title: "版本", symbol: "square.stack.3d.up.fill"),
        (title: "模组", symbol: "puzzlepiece.extension.fill"),
        (title: "光影", symbol: "sun.max.fill"),
        (title: "资源包", symbol: "square.grid.2x2.fill"),
        (title: "设置", symbol: "gearshape.fill")
    ]

    var body: some View {
        VStack(spacing: 0) {
            brand
                .padding(.horizontal, 18)
                .padding(.top, 14)
                .padding(.bottom, 8)
                .background {
                    NCLRSidebarEdgeBackground(canvas: palette.canvas, top: true)
                }
                .zIndex(1)

            ScrollViewReader { scroll in
                ScrollView(.vertical, showsIndicators: false) {
                    VStack(spacing: 2) {
                        ForEach(items.indices, id: \.self) { page in
                            navigationButton(for: page)
                                .padding(.top, page == 0 ? 32 : 0)
                                .padding(.bottom, page == items.count - 1 ? 32 : 0)
                                .id(page)
                        }
                    }
                    .backgroundPreferenceValue(NCLRSidebarRowBounds.self) { bounds in
                        GeometryReader { geometry in
                            if let anchor = bounds[state.selectedPage] {
                                let frame = geometry[anchor]
                                NCLRSidebarSelection(
                                    frame: frame, color: palette.selection,
                                    page: state.selectedPage,
                                    animated: state.animateSelection && !reduceMotion
                                )
                            }
                        }
                        .allowsHitTesting(false)
                        .accessibilityHidden(true)
                    }
                    .padding(.horizontal, 10)
                }
                .onAppear {
                    scroll.scrollTo(state.selectedPage)
                }
                .onChange(of: state.selectedPage) { page in
                    if items.indices.contains(page) {
                        var transaction = Transaction()
                        transaction.disablesAnimations = true
                        withTransaction(transaction) {
                            scroll.scrollTo(page)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            accountButton
                .padding(.horizontal, 12)
                .padding(.top, 8)
                .padding(.bottom, 8)
                .background {
                    NCLRSidebarEdgeBackground(canvas: palette.canvas, top: false)
                }
                .zIndex(1)
        }
        .ignoresSafeArea(.container, edges: .bottom)
        .buttonStyle(NCLRSidebarButtonStyle())
    }

    private var brand: some View {
        HStack(alignment: .top, spacing: 10) {
            Group {
                if let logo = UIImage(named: "BrandIcon") {
                    Image(uiImage: logo)
                        .resizable()
                        .scaledToFit()
                } else {
                    Text("N")
                        .font(.title2.bold())
                        .foregroundStyle(palette.accent)
                }
            }
            .frame(width: 34, height: 34)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))

            VStack(alignment: .leading, spacing: 2) {
                Text("NCLR")
                    .font(.system(size: 22, weight: .bold))
                    .foregroundStyle(palette.selectedForeground)
                Text("LAUNCHER")
                    .font(.system(size: 8, weight: .bold))
                    .tracking(2)
                    .foregroundStyle(palette.foreground)
            }
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("NCLR Launcher")
    }

    private func navigationButton(for page: Int) -> some View {
        let item = items[page]
        let selected = state.selectedPage == page
        return Button {
            selectionHandler(page)
        } label: {
            HStack(spacing: 14) {
                Image(systemName: item.symbol)
                    .font(.system(size: 22, weight: .semibold))
                    .frame(width: 26)
                Text(item.title)
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .foregroundStyle(selected ? palette.selectedForeground : palette.foreground)
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, minHeight: 52, alignment: .leading)
            .contentShape(Rectangle())
        }
        .anchorPreference(key: NCLRSidebarRowBounds.self, value: .bounds) { [page: $0] }
        .accessibilityLabel(item.title)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityIdentifier("nclr.sidebar.page.\(page)")
    }

    private var accountButton: some View {
        Button {
            selectionHandler(6)
        } label: {
            HStack(spacing: 10) {
                AsyncImage(url: state.account.avatarURL) { image in
                    image.resizable().interpolation(.none).scaledToFit()
                } placeholder: {
                    Text("N")
                        .font(.system(size: 15, weight: .bold))
                        .foregroundStyle(palette.accent)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(palette.accent.opacity(0.16), ignoresSafeAreaEdges: [])
                }
                .frame(width: 32, height: 32)
                .clipped()
                .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: 3) {
                    Text(state.account.name)
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(palette.selectedForeground)
                    Text(state.account.detail)
                        .font(.caption2)
                        .foregroundStyle(palette.foreground)
                }
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(minHeight: 52)
            .background {
                if state.selectedPage == 6 {
                    RoundedRectangle(cornerRadius: 11, style: .continuous)
                        .fill(palette.selection)
                }
            }
            .contentShape(Rectangle())
        }
        .accessibilityLabel("\(state.account.name)，\(state.account.detail)")
        .accessibilityHint("管理账号")
        .accessibilityAddTraits(state.selectedPage == 6 ? .isSelected : [])
        .accessibilityIdentifier("nclr.sidebar.account")
    }
}

private struct NCLRSidebarSelection: UIViewRepresentable {
    let frame: CGRect
    let color: Color
    let page: Int
    let animated: Bool

    func makeUIView(context: Context) -> NCLRSidebarSelectionView {
        NCLRSidebarSelectionView()
    }

    func updateUIView(_ view: NCLRSidebarSelectionView, context: Context) {
        view.update(frame: frame, color: UIColor(color), page: page, animated: animated)
    }
}

private final class NCLRSidebarSelectionView: UIView {
    private let highlight = CALayer()
    private var selectedPage: Int?

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        highlight.name = "nclr.sidebar.selection"
        highlight.cornerRadius = 11
        highlight.cornerCurve = .continuous
        layer.addSublayer(highlight)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("Use the selection initializer")
    }

    func update(frame: CGRect, color: UIColor, page: Int, animated: Bool) {
        let previous = highlight.presentation() ?? highlight
        let position = previous.position
        let bounds = previous.bounds
        // Anchor resolution can vary by a fraction of a pixel while scrolling.
        let currentFrame = highlight.frame
        let frameChanged = abs(currentFrame.minX - frame.minX) > 0.5
            || abs(currentFrame.minY - frame.minY) > 0.5
            || abs(currentFrame.width - frame.width) > 0.5
            || abs(currentFrame.height - frame.height) > 0.5
        let targetChanged = selectedPage != page || frameChanged
        let shouldAnimate = animated && selectedPage != nil && selectedPage != page
        selectedPage = page

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if targetChanged || !animated {
            highlight.removeAllAnimations()
            highlight.frame = frame
        }
        highlight.backgroundColor = color.resolvedColor(with: traitCollection).cgColor
        CATransaction.commit()

        guard targetChanged && shouldAnimate else { return }
        // Explicit layer animations keep moving while the destination page builds.
        let changes: [(String, NSValue, NSValue)] = [
            ("position", NSValue(cgPoint: position), NSValue(cgPoint: highlight.position)),
            ("bounds", NSValue(cgRect: bounds), NSValue(cgRect: highlight.bounds))
        ]
        for (key, start, end) in changes {
            let animation = CASpringAnimation(keyPath: key)
            animation.fromValue = start
            animation.toValue = end
            animation.stiffness = 280
            animation.damping = 26
            animation.duration = animation.settlingDuration
            let maximum = Float(window?.screen.maximumFramesPerSecond ?? 60)
            animation.preferredFrameRateRange = CAFrameRateRange(
                minimum: min(60, maximum), maximum: maximum, preferred: maximum
            )
            highlight.add(animation, forKey: key)
        }
    }
}

private struct NCLRSidebarEdgeBackground: View {
    let canvas: Color
    let top: Bool
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    var body: some View {
        GeometryReader { geometry in
            if reduceTransparency {
                canvas
            } else {
                let height = geometry.size.height + 32
                let fadeStart = geometry.size.height / height
                let fadeLength = 32 / height
                Rectangle()
                    .fill(
                        LinearGradient(
                            stops: [
                                .init(color: canvas, location: 0),
                                .init(color: canvas, location: fadeStart),
                                .init(color: canvas.opacity(0.844), location: fadeStart + fadeLength * 0.25),
                                .init(color: canvas.opacity(0.5), location: fadeStart + fadeLength * 0.5),
                                .init(color: canvas.opacity(0.156), location: fadeStart + fadeLength * 0.75),
                                .init(color: canvas.opacity(0), location: 1)
                            ],
                            startPoint: top ? .top : .bottom,
                            endPoint: top ? .bottom : .top
                        )
                    )
                    .frame(width: geometry.size.width, height: height)
                    .offset(y: top ? 0 : -32)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

private struct NCLRSidebarButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.6 : 1)
            .animation(.easeOut(duration: 0.1), value: configuration.isPressed)
    }
}
